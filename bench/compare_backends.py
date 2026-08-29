#!/usr/bin/env python3
"""Side-by-side ASR backend comparison on your own audio.

Benchmarks tell you how a model does on read audiobooks.  This tells you how it
does on the thing you actually watch.  It feeds the SAME segments through each
backend of `asr_srv.py` and prints the transcripts next to each other, so the
comparison reflects the real serving path — same HTTP endpoint, same chunk
boundaries, same language hint the app would use.

Segmentation mirrors `pipeline/chunker.rs` (graduated silence flush + 6 s cap)
so the audio each backend sees matches what the running app would send it.

Usage
-----
    python bench/compare_backends.py sample.mp4 --backends whisper-large,zipformer-ko
    python bench/compare_backends.py clip.wav --lang ko --out bench/out

Any format ffmpeg can read works; it is converted to 16 kHz mono internally.

Backends are named `<backend>[:<model>]`, with a few aliases pre-wired:

    whisper-turbo   deepdml/faster-whisper-large-v3-turbo-ct2   (float16)
    whisper-large   Systran/faster-whisper-large-v3             (int8_float16)
    whisper-large-fp16  same model at float16 (~3 GB VRAM, no quantization loss)
    whisper-medium  Systran/faster-whisper-medium               (float16)
    sensevoice      sherpa-onnx SenseVoice int8
    zipformer-ko    Korean Zipformer transducer

Requires: numpy (already an asr_srv.py dependency) and ffmpeg on PATH.
HTTP is done with the standard library so no extra install is needed.
"""

from __future__ import annotations

import argparse
import io
import json
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid
import wave
from dataclasses import dataclass, field
from pathlib import Path

try:
    import numpy as np
except ImportError as e:  # pragma: no cover - dependency hint
    sys.exit(f"missing dependency: {e}\n  pip install numpy")


SAMPLE_RATE = 16_000

# ── segmentation constants, mirrored from pipeline/chunker.rs ────────────────
SILENCE_RMS = 0.005          # ≈ −46 dBFS
BLOCK_SAMPLES = SAMPLE_RATE // 5      # 200 ms silence-detection block
CHUNK_CAP_SAMPLES = SAMPLE_RATE * 6   # 6 s hard cap
MIN_FLUSH_SAMPLES = SAMPLE_RATE // 2  # 0.5 s — don't emit near-empty audio


# `<alias>: (backend, model, extra asr_srv.py args)`
BACKEND_ALIASES: dict[str, tuple[str, str, list[str]]] = {
    "whisper-turbo": (
        "whisper", "deepdml/faster-whisper-large-v3-turbo-ct2", [],
    ),
    "whisper-large": (
        "whisper", "Systran/faster-whisper-large-v3", ["--compute-type", "int8_float16"],
    ),
    "whisper-large-fp16": (
        "whisper", "Systran/faster-whisper-large-v3", ["--compute-type", "float16"],
    ),
    "whisper-medium": (
        "whisper", "Systran/faster-whisper-medium", [],
    ),
    "sensevoice": (
        "sensevoice", "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17", [],
    ),
    "zipformer-ko": (
        "zipformer-ko", "", [],
    ),
}


# ── audio loading ────────────────────────────────────────────────────────────

def _read_wav_native(path: Path) -> np.ndarray | None:
    """Decode a plain PCM WAV without ffmpeg, if it is already 16 kHz mono.

    Covers the common case (a clip exported straight from the app's own
    pipeline) so the benchmark runs with nothing installed.  Anything else —
    a different sample rate, stereo, or compressed — goes through ffmpeg,
    because naive resampling would degrade the audio for every backend at once.
    """
    if path.suffix.lower() != ".wav":
        return None
    try:
        with wave.open(str(path), "rb") as w:
            if (w.getnchannels(), w.getsampwidth(), w.getframerate()) != (1, 2, SAMPLE_RATE):
                return None
            raw = w.readframes(w.getnframes())
    except (wave.Error, OSError):
        return None
    return np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0


def load_audio(path: Path) -> np.ndarray:
    """Decode `path` to float32 mono 16 kHz in [-1, 1]."""
    native = _read_wav_native(path)
    if native is not None:
        print("  (16 kHz mono WAV — decoded without ffmpeg)", flush=True)
        return native

    if shutil.which("ffmpeg") is None:
        sys.exit(
            "ffmpeg not found on PATH — needed for this input.\n"
            "  Install it:  winget install Gyan.FFmpeg\n"
            "  (then open a new terminal so PATH refreshes)\n"
            "  Or pass a 16 kHz mono 16-bit WAV, which is decoded natively."
        )

    with tempfile.TemporaryDirectory() as td:
        wav_path = Path(td) / "audio16k.wav"
        cmd = [
            "ffmpeg", "-nostdin", "-loglevel", "error", "-y",
            "-i", str(path),
            "-ac", "1", "-ar", str(SAMPLE_RATE), "-f", "wav", "-acodec", "pcm_s16le",
            str(wav_path),
        ]
        subprocess.run(cmd, check=True)
        with wave.open(str(wav_path), "rb") as w:
            raw = w.readframes(w.getnframes())

    pcm = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
    return pcm


def to_wav_bytes(samples: np.ndarray) -> bytes:
    """Encode float32 samples as a 16-bit mono 16 kHz WAV (in memory)."""
    clipped = np.clip(samples, -1.0, 1.0)
    pcm16 = (clipped * 32767.0).astype("<i2")
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(pcm16.tobytes())
    return buf.getvalue()


# ── segmentation (mirrors chunker.rs) ────────────────────────────────────────

def required_silence_blocks(buf_len: int) -> int:
    """How many consecutive silent 200 ms blocks end an utterance.

    Short buffer → demand a long pause, so a breath after 0.8 s doesn't produce
    a useless fragment.  Long buffer → cut at the first real dip.
    """
    if buf_len < SAMPLE_RATE * 3 // 2:
        return 4   # < 1.5 s of audio: need ≈ 800 ms of silence
    if buf_len < SAMPLE_RATE * 5 // 2:
        return 2   # 1.5 – 2.5 s: ≈ 400 ms
    return 1       # ≥ 2.5 s: first ≈ 200 ms dip is a good enough boundary


@dataclass
class Segment:
    index: int
    start_s: float
    end_s: float
    samples: np.ndarray

    @property
    def duration_s(self) -> float:
        return len(self.samples) / SAMPLE_RATE


def segment_audio(pcm: np.ndarray) -> list[Segment]:
    """Split into utterances the way the running app would."""
    segments: list[Segment] = []
    buf: list[np.ndarray] = []
    buf_len = 0
    silent_run = 0
    cursor = 0          # sample index of the start of the current buffer

    def flush(end_idx: int) -> None:
        nonlocal buf, buf_len, silent_run, cursor
        if buf_len >= MIN_FLUSH_SAMPLES:
            joined = np.concatenate(buf)
            segments.append(Segment(
                index=len(segments),
                start_s=cursor / SAMPLE_RATE,
                end_s=end_idx / SAMPLE_RATE,
                samples=joined,
            ))
        buf, buf_len, silent_run = [], 0, 0
        cursor = end_idx

    for pos in range(0, len(pcm) - BLOCK_SAMPLES + 1, BLOCK_SAMPLES):
        block = pcm[pos:pos + BLOCK_SAMPLES]
        level = float(np.sqrt(np.mean(block * block)))

        if buf_len == 0 and level < SILENCE_RMS:
            # Leading silence — don't start an utterance on it.
            cursor = pos + BLOCK_SAMPLES
            continue

        buf.append(block)
        buf_len += len(block)

        if level < SILENCE_RMS:
            silent_run += 1
            if silent_run >= required_silence_blocks(buf_len):
                flush(pos + BLOCK_SAMPLES)
                continue
        else:
            silent_run = 0

        if buf_len >= CHUNK_CAP_SAMPLES:
            flush(pos + BLOCK_SAMPLES)

    flush(len(pcm))
    return segments


# ── backend runner ───────────────────────────────────────────────────────────

@dataclass
class BackendResult:
    name: str
    model: str
    texts: list[str] = field(default_factory=list)
    langs: list[str] = field(default_factory=list)
    latencies_ms: list[float] = field(default_factory=list)
    load_s: float = 0.0
    error: str | None = None
    # Kept only on failure, so the report can point at the traceback.
    server_log: Path | None = None


def _encode_multipart(fields: dict[str, str], wav: bytes) -> tuple[bytes, str]:
    """Build a multipart/form-data body matching asr_srv.py's /inference form."""
    boundary = f"----bench{uuid.uuid4().hex}"
    parts: list[bytes] = []
    for name, value in fields.items():
        parts.append(
            f"--{boundary}\r\n"
            f'Content-Disposition: form-data; name="{name}"\r\n\r\n'
            f"{value}\r\n".encode()
        )
    parts.append(
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="chunk.wav"\r\n'
        f"Content-Type: audio/wav\r\n\r\n".encode()
    )
    parts.append(wav)
    parts.append(f"\r\n--{boundary}--\r\n".encode())
    return b"".join(parts), f"multipart/form-data; boundary={boundary}"


def post_inference(url: str, fields: dict[str, str], wav: bytes, timeout: float) -> dict:
    body, content_type = _encode_multipart(fields, wav)
    req = urllib.request.Request(url, data=body, method="POST")
    req.add_header("Content-Type", content_type)
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


def wait_for_ready(port: int, proc: subprocess.Popen, timeout_s: float) -> None:
    """Poll GET / until the server reports the model is loaded.

    asr_srv.py answers 503 while weights are still loading, 200 once ready.
    """
    url = f"http://127.0.0.1:{port}/"
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"asr_srv.py exited early (code {proc.returncode})")
        try:
            with urllib.request.urlopen(url, timeout=2) as resp:
                if resp.status == 200:
                    return
        except (urllib.error.URLError, urllib.error.HTTPError, OSError):
            pass  # not up yet, or still 503
        time.sleep(0.5)
    raise TimeoutError(f"model did not load within {timeout_s:.0f}s")


def run_backend(
    alias: str,
    segments: list[Segment],
    *,
    script: Path,
    python_bin: str,
    port: int,
    lang: str | None,
    load_timeout_s: float,
    log_dir: Path,
) -> BackendResult:
    if alias in BACKEND_ALIASES:
        backend, model, extra = BACKEND_ALIASES[alias]
    elif ":" in alias:
        backend, model = alias.split(":", 1)
        extra = []
    else:
        backend, model, extra = alias, "", []

    result = BackendResult(name=alias, model=model or "(server default)")

    cmd = [python_bin, str(script), "--backend", backend]
    if model:
        cmd += ["--model", model]
    cmd += ["--host", "127.0.0.1", "--port", str(port)] + extra

    print(f"\n── {alias} ──  {' '.join(cmd[1:])}", flush=True)
    # Keep the server's output: an HTTP 500 from /inference is meaningless
    # without the traceback behind it (missing CUDA DLLs, OOM, bad model id).
    log_dir.mkdir(parents=True, exist_ok=True)
    server_log = log_dir / f"{alias.replace(':', '_')}.server.log"
    log_handle = server_log.open("wb")
    proc = subprocess.Popen(cmd, stdout=log_handle, stderr=subprocess.STDOUT)
    try:
        t0 = time.monotonic()
        wait_for_ready(port, proc, load_timeout_s)
        result.load_s = time.monotonic() - t0
        print(f"   loaded in {result.load_s:.1f}s — {len(segments)} segments", flush=True)

        url = f"http://127.0.0.1:{port}/inference"
        for seg in segments:
            fields = {"response_format": "json", "beam_size": "1"}
            if lang:
                fields["language"] = lang

            t = time.monotonic()
            payload = post_inference(url, fields, to_wav_bytes(seg.samples), timeout=120)
            elapsed_ms = (time.monotonic() - t) * 1000

            result.texts.append((payload.get("text") or "").strip())
            result.langs.append(payload.get("language") or "")
            result.latencies_ms.append(elapsed_ms)
            print(f"   [{seg.index:>3}] {elapsed_ms:6.0f}ms  {result.texts[-1][:70]}", flush=True)
    except Exception as e:  # noqa: BLE001 — one bad backend shouldn't kill the run
        result.error = f"{type(e).__name__}: {e}"
        print(f"   FAILED — {result.error}", flush=True)
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            proc.kill()
        log_handle.close()

    if result.error:
        result.server_log = server_log
        # Surface the tail inline — the whole point of keeping it is to avoid
        # a second run just to find out why the first one failed.
        tail = _log_tail(server_log)
        if tail:
            print("   server log tail:", flush=True)
            for line in tail:
                print(f"     | {line}", flush=True)
    return result


def _log_tail(path: Path, lines: int = 12) -> list[str]:
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    return [ln for ln in text.splitlines() if ln.strip()][-lines:]


# ── reporting ────────────────────────────────────────────────────────────────

def write_report(
    out_dir: Path,
    source: Path,
    segments: list[Segment],
    results: list[BackendResult],
) -> Path:
    out_dir.mkdir(parents=True, exist_ok=True)
    ok = [r for r in results if not r.error]

    lines = [
        f"# ASR backend comparison — `{source.name}`",
        "",
        f"{len(segments)} segments · "
        f"{sum(s.duration_s for s in segments):.1f}s of speech · "
        f"segmentation mirrors `pipeline/chunker.rs`",
        "",
        "## Timing",
        "",
        "| backend | model | load | median | p90 | max |",
        "|---|---|---|---|---|---|",
    ]
    for r in results:
        if r.error:
            log_hint = f" (see `{r.server_log.name}`)" if r.server_log else ""
            lines.append(
                f"| {r.name} | {r.model} | — | — | — | FAILED: {r.error}{log_hint} |"
            )
            continue
        lat = sorted(r.latencies_ms)
        p90 = lat[min(len(lat) - 1, int(len(lat) * 0.9))] if lat else 0
        lines.append(
            f"| {r.name} | `{r.model}` | {r.load_s:.1f}s | "
            f"{statistics.median(lat):.0f}ms | {p90:.0f}ms | {max(lat):.0f}ms |"
        )

    lines += [
        "",
        "> Per-segment latency matters: the app sends a rolling partial 1 s in and",
        "> every 1.5 s after. A backend whose median exceeds ~1.5 s will have its",
        "> partials coalesced away, so the live preview thins out.",
        "",
        "## Transcripts",
        "",
    ]

    header = "| # | time | " + " | ".join(r.name for r in ok) + " |"
    lines += [header, "|" + "---|" * (len(ok) + 2)]
    for seg in segments:
        cells = []
        for r in ok:
            text = r.texts[seg.index] if seg.index < len(r.texts) else ""
            cells.append(text.replace("|", "\\|") or "_(silence)_")
        lines.append(
            f"| {seg.index} | {seg.start_s:.1f}–{seg.end_s:.1f}s | " + " | ".join(cells) + " |"
        )

    report = out_dir / "comparison.md"
    report.write_text("\n".join(lines) + "\n", encoding="utf-8")

    (out_dir / "comparison.json").write_text(
        json.dumps(
            {
                "source": str(source),
                "segments": [
                    {"index": s.index, "start_s": s.start_s, "end_s": s.end_s,
                     "duration_s": s.duration_s}
                    for s in segments
                ],
                "backends": [
                    {"name": r.name, "model": r.model, "error": r.error,
                     "load_s": r.load_s, "texts": r.texts, "langs": r.langs,
                     "latencies_ms": r.latencies_ms}
                    for r in results
                ],
            },
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    return report


# ── entry point ──────────────────────────────────────────────────────────────

def force_utf8_console() -> None:
    """Print Korean/Chinese transcripts on a legacy-codepage console.

    A Windows console inherits the system ANSI codepage (cp950 on a Traditional
    Chinese install), and printing Hangul raises UnicodeEncodeError — which
    would abort a backend AFTER its inference already succeeded.
    """
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, OSError):
            pass  # already UTF-8, or redirected somewhere that cannot reconfigure


def main() -> int:
    force_utf8_console()
    repo_root = Path(__file__).resolve().parent.parent

    ap = argparse.ArgumentParser(
        description="Compare asr_srv.py backends on the same audio.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="aliases: " + ", ".join(BACKEND_ALIASES),
    )
    ap.add_argument("audio", type=Path, help="audio or video file (anything ffmpeg reads)")
    ap.add_argument(
        "--backends", default="whisper-large,zipformer-ko",
        help="comma-separated aliases, or `backend:model` (default: %(default)s)",
    )
    ap.add_argument("--lang", default=None, help="language hint, e.g. ko (default: auto-detect)")
    ap.add_argument("--out", type=Path, default=repo_root / "bench" / "out")
    ap.add_argument("--port", type=int, default=9101, help="avoid the app's 9001")
    ap.add_argument("--python", default=sys.executable, help="python running asr_srv.py")
    ap.add_argument("--script", type=Path, default=repo_root / "asr_srv.py")
    ap.add_argument("--load-timeout", type=float, default=600.0,
                    help="seconds to wait for a model to load (first run downloads it)")
    ap.add_argument("--max-segments", type=int, default=0,
                    help="cap segments for a quick smoke run (0 = all)")
    args = ap.parse_args()

    if not args.audio.exists():
        sys.exit(f"no such file: {args.audio}")
    if not args.script.exists():
        sys.exit(f"asr_srv.py not found: {args.script}")

    print(f"decoding {args.audio} …", flush=True)
    pcm = load_audio(args.audio)
    print(f"  {len(pcm) / SAMPLE_RATE:.1f}s @ {SAMPLE_RATE} Hz mono", flush=True)

    segments = segment_audio(pcm)
    if not segments:
        sys.exit("no speech segments found — is the audio silent?")
    if args.max_segments:
        segments = segments[: args.max_segments]
    total = sum(s.duration_s for s in segments)
    print(f"  {len(segments)} segments, {total:.1f}s of speech "
          f"(mean {total / len(segments):.1f}s)", flush=True)

    results = [
        run_backend(
            alias.strip(), segments,
            script=args.script, python_bin=args.python, port=args.port,
            lang=args.lang, load_timeout_s=args.load_timeout, log_dir=args.out,
        )
        for alias in args.backends.split(",") if alias.strip()
    ]

    report = write_report(args.out, args.audio, segments, results)
    print(f"\nreport → {report}")
    if all(r.error for r in results):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
