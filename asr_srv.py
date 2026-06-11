#!/usr/bin/env python3
"""
ASR HTTP inference server — faster-whisper and SenseVoice (via sherpa-onnx) backends.

Exposes:
  GET  /            health check (returns 200 once model is loaded)
  POST /inference   same multipart fields as whisper.cpp server

Returns verbose_json that includes per-segment no_speech_prob so the
Rust caller can filter hallucinations on silence/noise chunks.

Usage:
  python asr_srv.py [--backend BACKEND] [--model MODEL] [--host HOST] [--port PORT]

  BACKEND  asr engine to use (env: ASR_BACKEND):
             whisper      faster-whisper  (default)
             sensevoice   SenseVoice ONNX via sherpa-onnx — better Korean accuracy

  MODEL  (whisper backend) HuggingFace repo id or local path (env: WHISPER_MODEL):
           Systran/faster-whisper-large-v3-turbo  ← recommended
           Systran/faster-whisper-large-v3        best quality (~3 GB VRAM)

         (sensevoice backend) HuggingFace repo id (env: SENSEVOICE_MODEL):
           csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17  ← default
"""

import argparse
import os
import sys
import tempfile

# ── DLL search path (Windows) ────────────────────────────────────────────────
if sys.platform == "win32":
    for _p in os.environ.get("PATH", "").split(os.pathsep):
        if _p and os.path.exists(os.path.join(_p, "cublas64_12.dll")):
            os.add_dll_directory(_p)
            print(f"[asr-srv] DLL path: {_p}", flush=True)
            break
    else:
        print("[asr-srv] WARN: cublas64_12.dll not found in PATH — GPU may not work", flush=True)

# ── FastAPI ──────────────────────────────────────────────────────────────────
try:
    from fastapi import FastAPI, Form, UploadFile
    import uvicorn
except ImportError as e:
    sys.exit(
        f"[asr-srv] Missing dependency: {e}\n"
        "Run: pip install 'uvicorn[standard]' fastapi python-multipart"
    )

app = FastAPI(title="asr-srv")
_ready: bool = False

# ── WAV helpers ──────────────────────────────────────────────────────────────

def _wav_to_float32(data: bytes):
    """Parse 16-bit PCM WAV bytes → float32 numpy array in [-1, 1].

    Assumes the format written by Rust's encode_wav_16bit:
    44-byte standard header, mono, 16 kHz, 16-bit signed PCM.
    """
    import numpy as np
    n = (len(data) - 44) // 2
    if n <= 0:
        return np.zeros(0, dtype=np.float32)
    return np.frombuffer(data, dtype="<i2", count=n, offset=44).astype(np.float32) / 32768.0

# ── backend: faster-whisper ──────────────────────────────────────────────────
_whisper_model = None

def _load_whisper(model_id: str, compute_type_override: str | None = None) -> None:
    global _whisper_model
    try:
        import ctranslate2
        from faster_whisper import WhisperModel
    except ImportError as e:
        sys.exit(
            f"[asr-srv] Missing faster-whisper dependency: {e}\n"
            "Run: pip install faster-whisper"
        )

    cuda_count = 0
    try:
        cuda_count = ctranslate2.get_cuda_device_count()
    except Exception:
        pass

    device = "cuda" if cuda_count > 0 else "cpu"
    if compute_type_override and compute_type_override != "auto":
        # int8_float16 needs CUDA; fall back to int8 on CPU
        compute_type = compute_type_override if (compute_type_override != "int8_float16" or cuda_count > 0) else "int8"
    else:
        compute_type = "float16" if cuda_count > 0 else "int8"

    print(f"[whisper] Loading model={model_id!r}  device={device}  compute={compute_type}", flush=True)
    print("[whisper] NOTE: first run downloads from HuggingFace — please wait", flush=True)
    _whisper_model = WhisperModel(model_id, device=device, compute_type=compute_type)


def _infer_whisper(
    tmp_path: str,
    language: str | None,
    prompt: str | None,
    beam_size: int,
) -> tuple[str, str, float]:
    lang = language if language and language not in ("auto", "") else None
    segments_iter, info = _whisper_model.transcribe(
        tmp_path,
        language=lang,
        initial_prompt=prompt or None,
        beam_size=beam_size,
        condition_on_previous_text=True,
        vad_filter=False,
        # Timestamps are discarded by the caller; skipping timestamp tokens is
        # faster and removes a hallucination source on short chunks.
        without_timestamps=True,
    )
    segs = list(segments_iter)
    text = "".join(s.text for s in segs)
    no_speech_prob = sum(s.no_speech_prob for s in segs) / len(segs) if segs else 0.0
    return text, info.language, float(no_speech_prob)

# ── backend: SenseVoice via sherpa-onnx ──────────────────────────────────────
# Recognizers are cached per language so that language hints work without
# reloading the model weights on every request.
_sv_recognizers: dict[str, object] = {}   # keyed by language code ("auto"|"ko"|"zh"|"en"|...)
_sv_model_path: str = ""
_sv_tokens_path: str = ""

# Events that indicate non-speech (music, applause, etc.)
_SV_NOISE_EVENTS = {"BGM", "Applause", "Laughter", "Cry", "Noise"}

# ISO codes accepted by sherpa-onnx SenseVoice language param
_SV_VALID_LANGS = {"zh", "en", "ko", "ja", "yue"}

def _download_sv_model(repo_id: str, precision: str = "int8") -> tuple[str, str]:
    """Download SenseVoice ONNX model files from HuggingFace, return (model_path, tokens_path).

    precision: "int8" (default, ~70 MB) | "fp32" (full precision, ~220 MB, better accuracy)
    """
    try:
        from huggingface_hub import hf_hub_download
    except ImportError:
        sys.exit(
            "[asr-srv] huggingface_hub not found.\n"
            "Run: pip install huggingface_hub"
        )
    model_file = "model.onnx" if precision == "fp32" else "model.int8.onnx"
    size_hint = "~220 MB" if precision == "fp32" else "~70 MB"
    print(f"[sensevoice] Downloading {model_file} from {repo_id!r} — first run only, {size_hint}", flush=True)
    model_path  = hf_hub_download(repo_id=repo_id, filename=model_file)
    tokens_path = hf_hub_download(repo_id=repo_id, filename="tokens.txt")
    return model_path, tokens_path


def _get_sv_recognizer(lang_hint: str | None):
    """Return a cached SenseVoice recognizer for the given language (create on first use)."""
    import sherpa_onnx
    lang_key = lang_hint if lang_hint in _SV_VALID_LANGS else "auto"
    if lang_key not in _sv_recognizers:
        print(f"[sensevoice] Creating recognizer for language={lang_key!r}", flush=True)
        _sv_recognizers[lang_key] = sherpa_onnx.OfflineRecognizer.from_sense_voice(
            model=_sv_model_path,
            tokens=_sv_tokens_path,
            language=lang_key,
            use_itn=True,
            num_threads=4,
            provider="cpu",
            debug=False,
        )
    return _sv_recognizers[lang_key]


def _load_sensevoice(model_id: str, precision: str = "int8") -> None:
    global _sv_model_path, _sv_tokens_path
    try:
        import sherpa_onnx  # noqa: F401
    except ImportError as e:
        sys.exit(
            f"[asr-srv] Missing sherpa-onnx dependency: {e}\n"
            "Run: pip install sherpa-onnx"
        )

    _sv_model_path, _sv_tokens_path = _download_sv_model(model_id, precision)
    print(f"[sensevoice] Loading model={_sv_model_path!r}  precision={precision}", flush=True)
    _get_sv_recognizer("auto")   # pre-warm default recognizer


def _infer_sensevoice(audio_bytes: bytes, language: str | None) -> tuple[str, str, float]:
    samples = _wav_to_float32(audio_bytes)
    if len(samples) == 0:
        return "", "auto", 0.9

    recognizer = _get_sv_recognizer(language)
    stream = recognizer.create_stream()
    stream.accept_waveform(sample_rate=16000, waveform=samples)
    recognizer.decode_stream(stream)

    result = stream.result
    text  = (result.text  or "").strip()
    lang  = result.lang   or "auto"   # e.g. "Korean", "Chinese", "English"
    event = result.event  or "Speech"

    # Non-speech events → suppress with high no_speech_prob
    no_speech_prob = 0.9 if (event in _SV_NOISE_EVENTS or not text) else 0.1
    return text, _normalize_lang(lang), no_speech_prob

# ── FastAPI endpoints ─────────────────────────────────────────────────────────

@app.get("/")
async def health():
    """Polled by Rust wait_for_server(); returns 200 only when model is ready."""
    if not _ready:
        from fastapi.responses import JSONResponse
        return JSONResponse({"status": "loading"}, status_code=503)
    return {"status": "ok"}


@app.post("/inference")
async def inference(
    file: UploadFile,
    response_format: str = Form("json"),
    initial_prompt: str | None = Form(None),
    language: str | None = Form(None),
    beam_size: int = Form(1),
):
    audio_data = await file.read()

    if _sv_model_path:
        # SenseVoice backend — initial_prompt and beam_size are not used
        text, lang, no_speech_prob = _infer_sensevoice(audio_data, language)
    else:
        # faster-whisper backend — needs a temp file path
        fd, tmp_path = tempfile.mkstemp(suffix=".wav")
        try:
            os.write(fd, audio_data)
            os.close(fd)
            text, lang, no_speech_prob = _infer_whisper(
                tmp_path, language, initial_prompt, beam_size
            )
        finally:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass

    return {
        "text": text,
        "language": lang,
        "segments": [
            {"text": text, "start": 0.0, "end": 0.0, "no_speech_prob": no_speech_prob}
        ] if text else [],
    }


def _normalize_lang(raw: str) -> str:
    """Map full language names and ISO codes to canonical 2-letter codes."""
    table = {
        # ISO codes (sherpa-onnx may return these when language is forced)
        "en": "en", "ko": "ko", "zh": "zh", "ja": "ja", "yue": "zh",
        # Full English names (sherpa-onnx SenseVoice result.lang)
        "english": "en", "korean": "ko", "chinese": "zh",
        "japanese": "ja", "cantonese": "zh",
        # faster-whisper ISO/name variants
        "eng": "en", "kor": "ko", "zho": "zh", "cmn": "zh",
        "jpn": "ja", "mandarin": "zh",
    }
    key = raw.strip().lower()
    if key in table:
        return table[key]
    if len(key) == 2:
        return key  # unknown 2-letter code, pass through
    print(f"[asr-srv] WARN: unknown language {raw!r}, defaulting to 'en'", flush=True)
    return "en"

# ── entry point ─────────────────────────────────────────────────────────────

def main() -> None:
    parser = argparse.ArgumentParser(description="ASR HTTP server (whisper / sensevoice)")
    parser.add_argument(
        "--backend", "-b",
        default=os.environ.get("ASR_BACKEND", "whisper"),
        choices=["whisper", "sensevoice"],
        help="ASR backend (env: ASR_BACKEND)",
    )
    parser.add_argument(
        "--model", "-m",
        default=None,
        help="Model id/path override (env: WHISPER_MODEL or SENSEVOICE_MODEL)",
    )
    parser.add_argument(
        "--compute-type", default=None,
        choices=["auto", "int8", "int8_float16", "float16", "float32"],
        help="Whisper compute/quantization type (auto = float16 GPU / int8 CPU)",
    )
    parser.add_argument(
        "--sv-precision", default="int8",
        choices=["int8", "fp32"],
        help="SenseVoice model precision: int8 (~70 MB, faster) | fp32 (~220 MB, more accurate)",
    )
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=9001)
    args = parser.parse_args()

    global _ready

    if args.backend == "sensevoice":
        model_id = (
            args.model
            or os.environ.get("SENSEVOICE_MODEL",
                              "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17")
        )
        _load_sensevoice(model_id, precision=args.sv_precision)
        print(f"[sensevoice] Ready on http://{args.host}:{args.port}", flush=True)
    else:
        model_id = (
            args.model
            or os.environ.get("WHISPER_MODEL", "Systran/faster-whisper-large-v3-turbo")
        )
        if model_id.endswith(".bin"):
            print(
                f"[asr-srv] WARN: WHISPER_MODEL={model_id!r} looks like a whisper.cpp file — "
                "faster-whisper needs a HuggingFace repo id. Falling back to large-v3-turbo.",
                flush=True,
            )
            model_id = "Systran/faster-whisper-large-v3-turbo"
        _load_whisper(model_id, compute_type_override=args.compute_type)
        print(f"[whisper] Ready on http://{args.host}:{args.port}", flush=True)

    _ready = True
    uvicorn.run(app, host=args.host, port=args.port, log_level="warning")


if __name__ == "__main__":
    main()
