#!/usr/bin/env python3
"""Sweep decode settings over one clip and rank them.

`compare_backends.py` answers "which backend?" by printing transcripts to read.
This answers "which *settings*?", where there are too many combinations to read
and the differences are too small to eyeball — you need an ordering.

What it measures and why those metrics, see `score.py`. In short: no ground
truth is required. `drift` compares each config against the same audio decoded
in one pass with no latency budget, `instability` catches the model inventing
by transcribing every chunk several times and measuring disagreement, and
`suspicious` counts known-bad shapes.

Usage
-----
    .venv/Scripts/python.exe bench/sweep.py bench/sample.wav --lang ko
    .venv/Scripts/python.exe bench/sweep.py music.wav --lang ko --repeats 5
    .venv/Scripts/python.exe bench/sweep.py clip.wav --only vad,rep-penalty

Configs sharing a model share one server process, so ordering the plan by model
is what keeps a sweep to minutes instead of one weight load per row.

The interpreter must be the `uv sync` venv — a system Python loads the model
and then 500s on every inference, because nvidia-cublas-cu12 lives in the venv.
"""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from compare_backends import (  # noqa: E402
    BACKEND_ALIASES,
    SAMPLE_RATE,
    force_utf8_console,
    load_audio,
    post_inference,
    segment_audio,
    to_wav_bytes,
    wait_for_ready,
)
from score import HEADER, ChunkResult, ConfigScore, score  # noqa: E402


# ── the plan ─────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Config:
    """One row of the sweep.

    `opts` goes to asr_srv.py's `decode_opts`, which whitelists what it accepts
    and otherwise keeps every production default — so a row differs from
    `baseline` only in what it names.
    """

    name: str
    backend: str = "whisper-large"
    opts: dict = field(default_factory=dict)
    beam_size: int = 5
    prompt: bool = True      # attach the app's rolling initial_prompt
    why: str = ""


# Ordered so configs sharing a backend sit together; the runner reuses servers.
PLAN: list[Config] = [
    Config("baseline", why="what the app runs today"),

    # ── A. decode knobs, in the order the evidence points ──
    Config("vad-filter", opts={"vad_filter": True},
           why="Silero VAD inside faster-whisper, currently off"),
    Config("rep-penalty", opts={"repetition_penalty": 1.15},
           why="kill decoder loops at the source, not after paying 2.7 s"),
    Config("no-repeat-3", opts={"no_repeat_ngram_size": 3},
           why="harder ban on repeats; may damage real repeated lyrics"),
    Config("halluc-silence", opts={"hallucination_silence_threshold": 2.0},
           why="faster-whisper's own guard, currently unset"),
    Config("no-prompt", prompt=False,
           why="ADR-0021: the prompt is what disarms the no_speech gate"),
    Config("no-condition", opts={"condition_on_previous_text": False},
           why="measured before: helps hallucination, doubles final latency"),
    Config("logprob-strict", opts={"log_prob_threshold": -0.7},
           why="drop low-confidence decodes the default keeps"),
    Config("compression-strict", opts={"compression_ratio_threshold": 2.0},
           why="trip the repetition guard earlier"),
    Config("no-temp-fallback", opts={"temperature": [0.0]},
           why="the six-temperature retry is where slow chunks go"),
    Config("beam1", beam_size=1, why="is beam 5 buying anything here?"),
    Config("vad+rep", opts={"vad_filter": True, "repetition_penalty": 1.15},
           why="the two most likely wins together"),

    # ── B. models ──
    Config("turbo", backend="whisper-turbo", why="the old default"),
    Config("large-fp16", backend="whisper-large-fp16",
           why="is int8 quantisation costing accuracy?"),
    Config("sensevoice", backend="sensevoice", why="CPU, Korean-strong"),
    Config("zipformer-ko", backend="zipformer-ko", why="Korean specialist"),
]

# The yardstick: whole file, one pass, best model, every accuracy option on and
# no latency budget. Not ground truth — a fixed point to measure drift from.
REFERENCE = Config(
    "reference(1-pass)", backend="whisper-large-fp16", beam_size=5, prompt=False,
    opts={"condition_on_previous_text": True},
    why="whole file in one request; the ceiling this audio allows",
)


# ── running ──────────────────────────────────────────────────────────────────

PROMPT_CHARS = 200  # mirrors the rolling prompt budget in asr/http_client.rs


class Server:
    """One asr_srv.py process, reused by every config on the same backend."""

    def __init__(self, alias: str, *, script: Path, python_bin: str, port: int,
                 load_timeout: float, log_dir: Path):
        backend, model, extra = BACKEND_ALIASES[alias]
        self.alias, self.port = alias, port
        self.model = model
        cmd = [python_bin, str(script), "--backend", backend, "--port", str(port)]
        if model:
            cmd += ["--model", model]
        cmd += extra
        self.log_path = log_dir / f"sweep.{alias}.log"
        log_dir.mkdir(parents=True, exist_ok=True)
        self._log = self.log_path.open("w", encoding="utf-8", errors="replace")
        t0 = time.monotonic()
        self.proc = subprocess.Popen(cmd, stdout=self._log, stderr=subprocess.STDOUT)
        wait_for_ready(port, self.proc, load_timeout)
        self.load_s = time.monotonic() - t0

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}/inference"

    def close(self) -> None:
        self.proc.terminate()
        try:
            self.proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        self._log.close()


def run_config(server: Server, cfg: Config, segments, *, lang, repeats, timeout):
    """Transcribe every segment `repeats` times, carrying a rolling prompt."""
    results: list[ChunkResult] = []
    rolling = ""  # what the app would have in initial_prompt by now
    for seg in segments:
        wav = to_wav_bytes(seg.samples)
        r = ChunkResult(index=seg.index, seconds=len(seg.samples) / SAMPLE_RATE)
        for _ in range(repeats):
            fields = {
                "response_format": "json",
                "beam_size": str(cfg.beam_size),
                "decode_opts": json.dumps(cfg.opts),
            }
            if lang:
                fields["language"] = lang
            if cfg.prompt and rolling:
                fields["initial_prompt"] = rolling
            t0 = time.monotonic()
            try:
                data = post_inference(server.url, fields, wav, timeout)
            except Exception as e:  # a row that errors must not kill the sweep
                r.runs.append("")
                r.langs.append("")
                r.no_speech.append(0.0)
                r.ms.append((time.monotonic() - t0) * 1000)
                print(f"    ! {cfg.name} seg{seg.index}: {e}", file=sys.stderr)
                continue
            r.ms.append((time.monotonic() - t0) * 1000)
            text = (data.get("text") or "").strip()
            r.runs.append(text)
            r.langs.append(data.get("language") or "")
            segs = data.get("segments") or []
            r.no_speech.append(segs[0].get("no_speech_prob", 0.0) if segs else 0.0)
        # Only the first run feeds the prompt: the app makes one request per
        # chunk, and letting repeat 5 set the context would measure a pipeline
        # that does not exist.
        if r.text:
            rolling = (rolling + " " + r.text)[-PROMPT_CHARS:]
        results.append(r)
    return results


def build_reference(pcm, *, script, python_bin, port, lang, load_timeout, timeout, log_dir):
    """Transcribe the whole clip in a single request, for `drift` to score against."""
    server = Server(REFERENCE.backend, script=script, python_bin=python_bin,
                    port=port, load_timeout=load_timeout, log_dir=log_dir)
    try:
        fields = {
            "response_format": "json",
            "beam_size": str(REFERENCE.beam_size),
            "decode_opts": json.dumps(REFERENCE.opts),
        }
        if lang:
            fields["language"] = lang
        data = post_inference(server.url, fields, to_wav_bytes(pcm), timeout)
        return (data.get("text") or "").strip()
    finally:
        server.close()


# ── report ───────────────────────────────────────────────────────────────────


def write_report(out_dir: Path, clip: Path, scores, plan, reference, segments,
                 repeats, chunk_count: int | None = None, transcripts=None):
    out_dir.mkdir(parents=True, exist_ok=True)
    why = {c.name: c.why for c in plan}
    # Drift first, not instability. With beam search at temperature 0 a decode
    # is deterministic, so instability only moves on chunks that tripped the
    # temperature fallback and got sampled — a real signal, but a sparse one
    # that leaves most rows tied at 0.000 and unrankable.
    ranked = sorted(scores, key=lambda s: (
        1.0 if s.drift is None else s.drift, s.suspicion_rate, s.instability))

    lines = [
        f"# Decode sweep — `{clip.name}`",
        "",
        (f"{len(segments)} segments · "
         f"{sum(len(s.samples) for s in segments) / SAMPLE_RATE:.1f}s of speech "
         f"· {repeats} run(s) per chunk")
        if segments is not None
        else f"{chunk_count} segments · {repeats} run(s) per chunk",
        "",
        "`drift` = CER against the same audio in one pass, no latency budget "
        "(not ground truth — a fixed yardstick). `instability` = mean pairwise "
        "CER across repeated runs of the same chunk; high means the model is "
        "guessing. Both lower is better. See `bench/score.py`.",
        "",
        "> **Read `drift` differently on music.** The yardstick is the same "
        "model given the whole file at once, which is a fair ceiling when there "
        "is speech to transcribe. Over singing it invents too, so a low drift "
        "there means *agrees with the reference's guess*, not *correct*. On "
        "music, `suspicious` and `instability` are the honest columns.",
        "",
    ]
    if repeats < 2:
        lines += ["> `instability` is 0 for every row: it needs `--repeats 2` "
                  "or more to mean anything.", ""]
    lines += ["## Ranked by drift", "", HEADER]
    lines += [s.row() for s in ranked]
    lines += ["", "## What each row changes", "",
              "| config | change |", "|---|---|"]
    lines += [f"| {c.name} | {why.get(c.name, '')} |" for c in plan]
    if reference:
        lines += ["", "## Reference transcript (single pass)", "",
                  "```text", reference, "```"]
    (out_dir / "sweep.md").write_text("\n".join(lines) + "\n", encoding="utf-8")

    (out_dir / "sweep.json").write_text(json.dumps({
        "clip": str(clip),
        "repeats": repeats,
        "reference": reference,
        "scores": [s.__dict__ for s in scores],
        # The aggregates alone cannot answer "what did it actually say?", which
        # is the first question any surprising row provokes — a 47% empty rate
        # is a win if the model declined to invent over an instrumental and a
        # bug if it dropped real singing, and the number is identical either
        # way. Keeping the text is the difference between a benchmark you can
        # audit and one you have to re-run.
        "transcripts": transcripts or {},
    }, ensure_ascii=False, indent=2), encoding="utf-8")


def rerender(out_dir: Path) -> int:
    """Rebuild sweep.md from a previous run's sweep.json.

    The numbers are the expensive part and they are already on disk; ranking
    and wording are not, and got re-argued twice before this existed.
    """
    src = out_dir / "sweep.json"
    if not src.exists():
        sys.exit(f"no {src} to re-render")
    blob = json.loads(src.read_text(encoding="utf-8"))
    scores = [ConfigScore(**d) for d in blob["scores"]]
    clip = Path(blob["clip"])
    # Segment count is only used for the header line; recover it from the
    # scores rather than re-decoding the audio just to count.
    chunks = max((s.chunks for s in scores), default=0)
    write_report(out_dir, clip, scores, PLAN, blob.get("reference"),
                 None, blob.get("repeats", 1), chunk_count=chunks,
                 transcripts=blob.get("transcripts"))
    print(f"re-rendered {out_dir / 'sweep.md'} from {chunks} chunks")
    return 0


def main() -> int:
    force_utf8_console()
    repo_root = Path(__file__).resolve().parent.parent
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("audio", type=Path)
    ap.add_argument("--lang", default=None, help="language hint, e.g. ko")
    ap.add_argument("--repeats", type=int, default=3,
                    help="runs per chunk; 1 disables the instability metric")
    ap.add_argument("--only", default="", help="comma-separated config names")
    ap.add_argument("--skip-reference", action="store_true",
                    help="skip the single-pass reference (drift will be blank)")
    ap.add_argument("--out", type=Path, default=repo_root / "bench" / "out")
    ap.add_argument("--port", type=int, default=9105, help="avoid 9001/9101/9103")
    ap.add_argument("--python", default=sys.executable)
    ap.add_argument("--script", type=Path, default=repo_root / "asr_srv.py")
    ap.add_argument("--load-timeout", type=float, default=600.0)
    ap.add_argument("--timeout", type=float, default=180.0)
    ap.add_argument("--max-segments", type=int, default=0)
    ap.add_argument("--report-only", action="store_true",
                    help="re-render sweep.md from an existing sweep.json; "
                         "changes ranking or wording without paying for inference again")
    args = ap.parse_args()

    if args.report_only:
        return rerender(args.out)

    plan = PLAN
    if args.only:
        want = {n.strip() for n in args.only.split(",") if n.strip()}
        plan = [c for c in PLAN if c.name in want]
        missing = want - {c.name for c in plan}
        if missing:
            sys.exit(f"unknown config(s): {', '.join(sorted(missing))}\n"
                     f"available: {', '.join(c.name for c in PLAN)}")
    if not plan:
        sys.exit("nothing to run")

    pcm = load_audio(args.audio)
    segments = segment_audio(pcm)
    if args.max_segments:
        segments = segments[:args.max_segments]
    if not segments:
        sys.exit("no audio segments found")
    print(f"{len(segments)} segments, {len(pcm)/SAMPLE_RATE:.1f}s, "
          f"{len(plan)} configs × {args.repeats} run(s)")

    reference = None
    if not args.skip_reference:
        print(f"reference: {REFERENCE.backend}, single pass...")
        try:
            reference = build_reference(
                pcm, script=args.script, python_bin=args.python, port=args.port,
                lang=args.lang, load_timeout=args.load_timeout,
                timeout=args.timeout, log_dir=args.out)
            print(f"  {len(reference)} chars")
        except Exception as e:
            print(f"  reference failed ({e}) — drift will be blank", file=sys.stderr)

    scores = []
    server = None
    current = None
    transcripts: dict[str, list[str]] = {}
    try:
        for cfg in sorted(plan, key=lambda c: c.backend):
            if cfg.backend != current:
                if server:
                    server.close()
                print(f"loading {cfg.backend}...")
                server = Server(cfg.backend, script=args.script,
                                python_bin=args.python, port=args.port,
                                load_timeout=args.load_timeout, log_dir=args.out)
                current = cfg.backend
                print(f"  ready in {server.load_s:.1f}s")
            t0 = time.monotonic()
            results = run_config(server, cfg, segments, lang=args.lang,
                                 repeats=args.repeats, timeout=args.timeout)
            s = score(cfg.name, results, reference)
            scores.append(s)
            transcripts[cfg.name] = [r.text for r in results]
            print(f"  {cfg.name:<20} drift={('—' if s.drift is None else f'{s.drift:.3f}')} "
                  f"instab={s.instability:.3f} susp={s.suspicion_rate:.0%} "
                  f"median={s.median_ms:.0f}ms  ({time.monotonic()-t0:.0f}s)")
    finally:
        if server:
            server.close()

    write_report(args.out, args.audio, scores, plan, reference, segments,
                 args.repeats, transcripts=transcripts)
    print(f"\nwrote {args.out / 'sweep.md'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
