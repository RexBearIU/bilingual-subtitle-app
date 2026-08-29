"""Reproduce the app's rolling partial chunks and measure what leaks through.

`compare_backends.py` sends one request per utterance. The app does not: it
flushes a partial after 1 s and refreshes every 1.5 s, and attaches a rolling
`initial_prompt`. Hallucinations live almost entirely in those short prompted
requests, so measuring them needs a harness that reproduces both.

That distinction is not cosmetic. The same 1.2 s chunk, transcribed `Bye.` in
the middle of Korean audio, scores `no_speech_prob` 0.902 on its own and 0.000
once the prompt is attached — which is why the app's `no_speech >= 0.7` filter
almost never fires on the chunks it exists for (ADR-0021).

Prints one line per request, marking the ones the app's current rule would
suppress, and writes the raw measurements to `bench/out/partials.json` so a
threshold can be re-evaluated without re-running inference.

Audio goes to the ASR server over HTTP. Nothing is played through the speakers.

    python bench/probe_partials.py bench/sample.wav
    python bench/probe_partials.py some_clip.mp4 --lang ko --backend whisper-turbo
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from collections import Counter, deque
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import compare_backends as cb

SR = 16_000

# Chunker cadence, mirroring pipeline/chunker.rs.
FIRST_PARTIAL_S = 1.0
REFRESH_S = 1.5

# The suppression rule, mirroring asr/http_client.rs. Keep these in step with
# LANG_WINDOW / SHORT_FLIP_SECS there, or the verdicts below mean nothing.
LANG_WINDOW = 5
SHORT_FLIP_SECS = 2.0


def established_lang(window: deque[str]) -> str | None:
    """The language being spoken: a strict majority of recent accepted finals.

    Not "the last final's language" — one hallucination that gets through
    would become the reference, and every correct line after it would then
    read as the flip.
    """
    if not window:
        return None
    lang, n = Counter(window).most_common(1)[0]
    return lang if n * 2 > len(window) else None


def partial_cuts(n_samples: int) -> list[int]:
    """Prefix lengths the chunker would have flushed, ending with the final."""
    cuts: list[int] = []
    t = FIRST_PARTIAL_S
    while int(t * SR) < n_samples:
        cuts.append(int(t * SR))
        t += REFRESH_S
    cuts.append(n_samples)
    return cuts


def main() -> int:
    cb.force_utf8_console()
    repo_root = Path(__file__).resolve().parent.parent

    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument("audio", type=Path, help="audio or video file (anything ffmpeg reads)")
    ap.add_argument("--backend", default="whisper:large",
                    help="an alias from compare_backends.BACKEND_ALIASES, or backend:model. "
                         "Defaults to what the app itself runs (asr_srv's own 'large'), so "
                         "the numbers describe the app rather than a benchmark variant")
    ap.add_argument("--lang", default=None,
                    help="language hint; omit to let the model auto-detect, which is "
                         "what surfaces the language flips this tool looks for")
    ap.add_argument("--out", type=Path, default=repo_root / "bench" / "out")
    ap.add_argument("--port", type=int, default=9103, help="avoid the app's 9001")
    ap.add_argument("--python", default=sys.executable,
                    help="python running asr_srv.py — must be the uv-created .venv, "
                         "or CUDA is missing and every inference returns 500")
    ap.add_argument("--script", type=Path, default=repo_root / "asr_srv.py")
    ap.add_argument("--load-timeout", type=float, default=600.0)
    ap.add_argument("--no-fast-partials", action="store_true",
                    help="send partials WITHOUT the server's `fast` flag, to measure what "
                         "faster-whisper's temperature fallback costs on repetitive audio")
    args = ap.parse_args()

    if not args.audio.exists():
        print(f"no such file: {args.audio}", file=sys.stderr)
        return 2

    pcm = cb.load_audio(args.audio)
    segments = cb.segment_audio(pcm)
    print(f"{len(segments)} segments from {args.audio.name}", flush=True)

    alias = args.backend
    if alias in cb.BACKEND_ALIASES:
        backend, model, extra = cb.BACKEND_ALIASES[alias]
    elif ":" in alias:
        backend, model, extra = *alias.split(":", 1), []
    else:
        backend, model, extra = alias, "", []

    cmd = [args.python, str(args.script), "--backend", backend]
    if model:
        cmd += ["--model", model]
    cmd += ["--host", "127.0.0.1", "--port", str(args.port)] + extra

    args.out.mkdir(parents=True, exist_ok=True)
    server_log = args.out / "partials.server.log"
    log_handle = server_log.open("wb")
    proc = subprocess.Popen(cmd, stdout=log_handle, stderr=subprocess.STDOUT,
                            cwd=str(repo_root))

    rows: list[dict] = []
    try:
        cb.wait_for_ready(args.port, proc, args.load_timeout)
        url = f"http://127.0.0.1:{args.port}/inference"
        print(f"{alias} ready — {sum(len(partial_cuts(len(s.samples))) for s in segments)} "
              f"requests\n", flush=True)

        prompt: str | None = None
        window: deque[str] = deque(maxlen=LANG_WINDOW)

        for seg in segments:
            cuts = partial_cuts(len(seg.samples))
            for k, end in enumerate(cuts):
                is_partial = k < len(cuts) - 1
                fields = {"response_format": "verbose_json", "beam_size": "1"}
                # Partials ask the server to skip the temperature-fallback
                # retries, exactly as the app does.
                if is_partial and not args.no_fast_partials:
                    fields["fast"] = "true"
                if args.lang:
                    fields["language"] = args.lang
                if prompt:
                    fields["initial_prompt"] = prompt

                t = time.monotonic()
                payload = cb.post_inference(
                    url, fields, cb.to_wav_bytes(seg.samples[:end]), timeout=120
                )
                ms = (time.monotonic() - t) * 1000

                text = (payload.get("text") or "").strip()
                lang = payload.get("language") or ""
                probs = [s["no_speech_prob"] for s in (payload.get("segments") or [])
                         if "no_speech_prob" in s]
                no_speech = sum(probs) / len(probs) if probs else -1.0
                secs = end / SR

                est = established_lang(window)
                suppressed = est is not None and lang != est and secs <= SHORT_FLIP_SECS

                rows.append({
                    "segment": seg.index, "step": k, "partial": is_partial,
                    "seconds": round(secs, 2), "ms": round(ms),
                    "language": lang, "no_speech": round(no_speech, 3),
                    "established": est, "suppressed": suppressed,
                    "text": text, "prompt": prompt or "",
                })

                mark = "P" if is_partial else "F"
                verdict = "  << suppressed" if suppressed else ""
                print(f"[{seg.index:>3}.{k}{mark}] {secs:>5.2f}s  ns={no_speech:6.3f}  "
                      f"lang={lang:<3} {text[:52]}{verdict}", flush=True)

                if text:
                    prompt = text[-200:]
                    # Only an ACCEPTED final sets the reference. A suppressed
                    # chunk must not, or one bad line poisons the window.
                    if not is_partial and not suppressed:
                        window.append(lang)
    except Exception as e:  # noqa: BLE001 — report what was collected either way
        print(f"FAILED — {type(e).__name__}: {e}", file=sys.stderr)
        print(f"server log: {server_log}", file=sys.stderr)
    finally:
        proc.terminate()
        log_handle.close()

    out = args.out / "partials.json"
    out.write_text(json.dumps(rows, ensure_ascii=False, indent=1), encoding="utf-8")

    hits = [r for r in rows if r["suppressed"]]
    print(f"\n{len(rows)} requests, {len(hits)} suppressed by the current rule "
          f"(<= {SHORT_FLIP_SECS}s and a language flip)")
    print("Check every suppressed line above: each one should be a hallucination,")
    print("and no real speech should be missing from the kept lines.")
    print(f"\nwrote {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
