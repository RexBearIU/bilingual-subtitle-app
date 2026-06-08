#!/usr/bin/env python3
"""
faster-whisper HTTP inference server — drop-in for whisper.cpp --server.

Exposes:
  GET  /            health check (returns 200 once model is loaded)
  POST /inference   same multipart fields as whisper.cpp server

Returns verbose_json that includes per-segment no_speech_prob so the
Rust caller can filter hallucinations on silence/noise chunks.

Usage:
  python faster_whisper_srv.py [--model MODEL] [--host HOST] [--port PORT]

  MODEL  HuggingFace repo id or local path.
         Default: Systran/faster-whisper-medium  (~1.5 GB, downloads on first run)
         Other options: Systran/faster-whisper-small (faster, ~500 MB)
                        Systran/faster-whisper-large-v3 (best quality, ~3 GB)
"""

import argparse
import os
import sys
import tempfile

# ── DLL search path (Windows) ────────────────────────────────────────────────
# The Rust launcher prepends the sidecar DLL directory to PATH before spawning
# this process, so ctranslate2 can find cublas64_12.dll and friends.
# We just need to call os.add_dll_directory() for each PATH entry that contains
# the DLLs (os.add_dll_directory is needed for delay-loaded DLLs on Python 3.8+).
if sys.platform == "win32":
    for _p in os.environ.get("PATH", "").split(os.pathsep):
        if _p and os.path.exists(os.path.join(_p, "cublas64_12.dll")):
            os.add_dll_directory(_p)
            print(f"[faster-whisper] DLL path: {_p}", flush=True)
            break
    else:
        print("[faster-whisper] WARN: cublas64_12.dll not found in PATH — GPU may not work", flush=True)

# ── dependency check ────────────────────────────────────────────────────────
try:
    import ctranslate2
    from fastapi import FastAPI, Form, UploadFile
    from faster_whisper import WhisperModel
    import uvicorn
except ImportError as e:
    sys.exit(
        f"[faster-whisper] Missing dependency: {e}\n"
        "Run: pip install faster-whisper 'uvicorn[standard]' fastapi python-multipart"
    )

# ── FastAPI app ─────────────────────────────────────────────────────────────
app = FastAPI(title="faster-whisper-srv")
_model: WhisperModel | None = None
_ready: bool = False


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

    # Write to temp file (faster-whisper needs a file path, not bytes).
    fd, tmp_path = tempfile.mkstemp(suffix=".wav")
    try:
        os.write(fd, audio_data)
        os.close(fd)

        # Normalize language: pass None for "auto" so whisper auto-detects.
        lang = language if language and language not in ("auto", "") else None

        segments_iter, info = _model.transcribe(
            tmp_path,
            language=lang,
            initial_prompt=initial_prompt or None,
            beam_size=beam_size,
            vad_filter=False,   # VAD is handled by the Rust pipeline
        )
        segs = list(segments_iter)
        text = "".join(s.text for s in segs)

        # Always return verbose_json so the caller gets no_speech_prob.
        return {
            "text": text,
            "language": info.language,
            "segments": [
                {
                    "text": s.text,
                    "start": s.start,
                    "end": s.end,
                    "no_speech_prob": s.no_speech_prob,
                }
                for s in segs
            ],
        }
    finally:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass


# ── entry point ─────────────────────────────────────────────────────────────
def main() -> None:
    parser = argparse.ArgumentParser(description="faster-whisper HTTP server")
    parser.add_argument(
        "--model", "-m",
        default="Systran/faster-whisper-medium",
        help="HuggingFace model id or local path (default: Systran/faster-whisper-medium)",
    )
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=9001)
    args = parser.parse_args()

    # Device: prefer CUDA (fp16) for GPU speed; fall back to CPU int8.
    cuda_count = 0
    try:
        cuda_count = ctranslate2.get_cuda_device_count()
    except Exception:
        pass

    if cuda_count > 0:
        device, compute_type = "cuda", "float16"
    else:
        device, compute_type = "cpu", "int8"

    print(
        f"[faster-whisper] Loading model={args.model!r}  "
        f"device={device}  compute={compute_type}",
        flush=True,
    )
    print(
        "[faster-whisper] NOTE: first run downloads ~1.5 GB from HuggingFace — please wait",
        flush=True,
    )

    global _model, _ready
    _model = WhisperModel(args.model, device=device, compute_type=compute_type)
    _ready = True
    print(
        f"[faster-whisper] Ready on http://{args.host}:{args.port}",
        flush=True,
    )

    uvicorn.run(app, host=args.host, port=args.port, log_level="warning")


if __name__ == "__main__":
    main()
