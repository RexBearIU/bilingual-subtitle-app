#!/usr/bin/env python3
"""Side-by-side translation comparison across OpenRouter models.

The counterpart to `compare_backends.py`: that one answers "which ASR hears the
audio best", this one answers "which model turns that text into subtitles best".

It feeds real ASR output — by default the transcripts `compare_backends.py`
already produced — through each model using the SAME system prompt and request
shape as `src-tauri/src/translate/openrouter.rs`, so the numbers reflect the
serving path rather than a hand-written prompt.

What it measures, beyond the obvious:

- **Latency**, which for subtitles is the binding constraint, not quality.
- **Reasoning contamination.** Models that think before answering spend the
  token budget on reasoning and can return empty content, or leak the thinking
  into the subtitle. Both are recorded.
- **Rate-limit reality.** `:free` models share a pool and return 429 under any
  sustained load. One retry per call keeps the picture fair without hiding it.

Usage
-----
    python bench/compare_translation.py --free-all
    python bench/compare_translation.py --models google/gemini-2.5-flash-lite,z-ai/glm-5.3-flash

Reads the API key from OPENROUTER_API_KEY or a local .env. Never prints it.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

API_ROOT = "https://openrouter.ai/api/v1"

# Mirrors build_system_prompt() in translate/openrouter.rs for source_lang="ko".
SYSTEM_PROMPT = (
    "You are a real-time subtitle translator. "
    "Output ONLY the {target} translation — no explanations, no additions. "
    "Keep the natural spoken tone. "
    "The input comes from live speech recognition: it may contain transcription "
    "errors, odd spacing, or be a fragment cut mid-sentence. Infer the intended "
    "words from context, translate only what is present, and never invent "
    "content to complete a fragment. "
    "The audio may contain several speakers taking turns — if the text switches "
    "speaker mid-line (often marked with a dash), translate each utterance and "
    "keep them separated with a dash; do not merge different speakers into one "
    "sentence. "
    "For Korean: keep English loanwords in English, transliterate proper names "
    "phonetically, match the speaker's formal or casual register."
)


def force_utf8_console() -> None:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, OSError):
            pass


def load_env(path: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return out
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.removeprefix("export ").split("=", 1)
        value = value.strip().strip('"').strip("'")
        if value:
            out[key.strip()] = value
    return out


def api_key(repo_root: Path) -> str:
    key = os.environ.get("OPENROUTER_API_KEY") or load_env(repo_root / ".env").get(
        "OPENROUTER_API_KEY", ""
    )
    if not key:
        sys.exit("no OPENROUTER_API_KEY (env or .env)")
    return key


def get_json(url: str, key: str | None = None, timeout: float = 30) -> dict:
    req = urllib.request.Request(url)
    if key:
        req.add_header("Authorization", f"Bearer {key}")
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read())


def free_models(key: str) -> list[str]:
    data = get_json(f"{API_ROOT}/models")["data"]
    return sorted(m["id"] for m in data if str(m["id"]).endswith(":free"))


@dataclass
class ModelResult:
    model: str
    outputs: list[str] = field(default_factory=list)
    latencies_ms: list[float] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    empty: int = 0
    reasoning_chars: int = 0
    reasoning_required: bool = False

    @property
    def ok(self) -> int:
        return len(self.latencies_ms)


def translate_once(
    key: str, model: str, text: str, target: str, timeout: float
) -> tuple[float, str, int, bool]:
    """Return (latency_ms, content, reasoning_chars, reasoning_required)."""
    body = {
        "model": model,
        "max_tokens": 200,
        "temperature": 0,
        "reasoning": {"enabled": False},
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT.format(target=target)},
            {"role": "user", "content": f"[Korean→{target}] {text}"},
        ],
    }
    reasoning_required = False

    for attempt in range(3):
        payload = json.dumps(body).encode()
        req = urllib.request.Request(f"{API_ROOT}/chat/completions", data=payload, method="POST")
        req.add_header("Authorization", f"Bearer {key}")
        req.add_header("Content-Type", "application/json")
        req.add_header("HTTP-Referer", "https://github.com/RexBearIU/bilingual-subtitle-app")
        req.add_header("X-Title", "Bilingual Subtitles")

        started = time.monotonic()
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                data = json.loads(resp.read())
            elapsed = (time.monotonic() - started) * 1000
            message = (data.get("choices") or [{}])[0].get("message", {}) or {}
            content = (message.get("content") or "").strip()
            reasoning = message.get("reasoning") or ""
            return elapsed, content, len(reasoning), reasoning_required
        except urllib.error.HTTPError as e:
            detail = e.read()[:400].decode(errors="replace")
            # Same fallback as post_with_retry() in openrouter.rs.
            if e.code == 400 and "reasoning" in detail.lower() and "reasoning" in body:
                body.pop("reasoning")
                reasoning_required = True
                continue
            if e.code == 429 and attempt < 2:
                time.sleep(2.0)
                continue
            raise RuntimeError(f"HTTP {e.code}: {detail[:120]}") from None
    raise RuntimeError("retries exhausted")


def run_model(key: str, model: str, texts: list[str], target: str, timeout: float) -> ModelResult:
    result = ModelResult(model=model)
    print(f"\n── {model}", flush=True)
    for text in texts:
        try:
            ms, content, rchars, required = translate_once(key, model, text, target, timeout)
        except Exception as e:  # noqa: BLE001 — one bad model must not stop the survey
            msg = str(e).replace("\n", " ")[:110]
            result.errors.append(msg)
            print(f"   FAIL  {msg}", flush=True)
            continue
        result.latencies_ms.append(ms)
        result.outputs.append(content)
        result.reasoning_chars += rchars
        result.reasoning_required |= required
        if not content:
            result.empty += 1
        flag = "  [reasoning]" if rchars else ""
        print(f"   {ms:7.0f}ms{flag}  {content[:44] or '(empty)'}", flush=True)
    return result


def write_report(out_dir: Path, results: list[ModelResult], texts: list[str]) -> Path:
    out_dir.mkdir(parents=True, exist_ok=True)
    usable = [r for r in results if r.ok and r.empty == 0 and not r.errors]

    lines = [
        "# Translation model comparison (Korean → Traditional Chinese)",
        "",
        f"{len(texts)} lines of real ASR output · same prompt and request shape as "
        "`translate/openrouter.rs`",
        "",
        "| model | ok/total | median | p90 | empty | reasoning | errors |",
        "|---|---|---|---|---|---|---|",
    ]
    for r in sorted(results, key=lambda x: (not x.ok, statistics.median(x.latencies_ms) if x.ok else 1e9)):
        if not r.ok:
            lines.append(
                f"| `{r.model}` | 0/{len(texts)} | — | — | — | — | {r.errors[0] if r.errors else 'no data'} |"
            )
            continue
        lat = sorted(r.latencies_ms)
        p90 = lat[min(len(lat) - 1, int(len(lat) * 0.9))]
        note = "mandatory" if r.reasoning_required else ("yes" if r.reasoning_chars else "no")
        lines.append(
            f"| `{r.model}` | {r.ok}/{len(texts)} | {statistics.median(lat):.0f}ms | "
            f"{p90:.0f}ms | {r.empty} | {note} | {len(r.errors)} |"
        )

    lines += [
        "",
        "> `empty` counts responses that arrived well-formed with no content — a",
        "> reasoning model spending the whole `max_tokens` budget before answering.",
        "> A model with any empty line is unusable regardless of its latency.",
        "",
        "## Output",
        "",
    ]
    if usable:
        lines += ["| # | " + " | ".join(f"`{r.model}`" for r in usable) + " |",
                  "|" + "---|" * (len(usable) + 1)]
        for i in range(len(texts)):
            cells = [
                (r.outputs[i] if i < len(r.outputs) else "").replace("|", "\\|") or "_(empty)_"
                for r in usable
            ]
            lines.append(f"| {i} | " + " | ".join(cells) + " |")
    else:
        lines.append("_No model completed every line cleanly._")

    report = out_dir / "translation.md"
    report.write_text("\n".join(lines) + "\n", encoding="utf-8")
    (out_dir / "translation.json").write_text(
        json.dumps(
            {
                "inputs": texts,
                "models": [
                    {
                        "model": r.model, "outputs": r.outputs, "latencies_ms": r.latencies_ms,
                        "errors": r.errors, "empty": r.empty,
                        "reasoning_chars": r.reasoning_chars,
                        "reasoning_required": r.reasoning_required,
                    }
                    for r in results
                ],
            },
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    return report


def main() -> int:
    force_utf8_console()
    repo_root = Path(__file__).resolve().parent.parent

    ap = argparse.ArgumentParser(description="Compare OpenRouter models on real ASR output.")
    ap.add_argument("--models", default="", help="comma-separated model slugs")
    ap.add_argument("--free-all", action="store_true", help="every `:free` model on OpenRouter")
    ap.add_argument("--baseline", default="google/gemini-2.5-flash-lite",
                    help="always included for comparison ('' to skip)")
    ap.add_argument("--asr-json", type=Path, default=repo_root / "bench" / "out" / "comparison.json")
    ap.add_argument("--asr-backend", default="whisper-turbo")
    ap.add_argument("--lines", type=int, default=4, help="how many ASR lines to translate")
    ap.add_argument("--target", default="Traditional Chinese")
    ap.add_argument("--timeout", type=float, default=45.0)
    ap.add_argument("--out", type=Path, default=repo_root / "bench" / "out")
    args = ap.parse_args()

    key = api_key(repo_root)

    if not args.asr_json.exists():
        sys.exit(f"no ASR output at {args.asr_json} — run compare_backends.py first")
    data = json.loads(args.asr_json.read_text(encoding="utf-8"))
    backend = next(
        (b for b in data["backends"] if b["name"] == args.asr_backend and not b.get("error")),
        None,
    )
    if backend is None:
        sys.exit(f"backend {args.asr_backend!r} not in {args.asr_json}")
    texts = [t for t in backend["texts"] if len(t.strip()) > 8][: args.lines]
    if not texts:
        sys.exit("no usable ASR lines found")
    print(f"{len(texts)} lines from {args.asr_backend}")

    models: list[str] = []
    if args.free_all:
        models += free_models(key)
    models += [m.strip() for m in args.models.split(",") if m.strip()]
    if args.baseline and args.baseline not in models:
        models.append(args.baseline)
    if not models:
        sys.exit("no models selected (use --models or --free-all)")
    print(f"{len(models)} models to test")

    results = [run_model(key, m, texts, args.target, args.timeout) for m in models]
    report = write_report(args.out, results, texts)
    print(f"\nreport → {report}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
