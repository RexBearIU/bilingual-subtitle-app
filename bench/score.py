#!/usr/bin/env python3
"""Scoring for ASR output, built for the case where there is no ground truth.

`compare_backends.py` deliberately does not score — it prints transcripts side
by side and lets you read them, which is right for two backends and hopeless
for twenty parameter combinations. This module supplies the numbers that make
a sweep rankable.

Three of the four metrics need no reference transcript, because for this app
there usually isn't one:

- Korean speech would have to be transcribed by hand.
- Korean lyrics are published, but a reference file cannot be built out of
  them here, and a benchmark that only works for songs you own the words to is
  not a benchmark.

What follows are the signals that survive that constraint.

## instability — the one that finds hallucinations

Whisper is not deterministic. Run the same chunk twice and *real* content comes
back the same; an invention comes back differently worded. Measured in this
repo: one silent chunk produced `¡Bienvenidos a la secundita!` on one run and
`¿Qué es esto?` on the next, while every chunk of actual speech was stable.

So: transcribe each chunk N times and measure how much the outputs disagree.
High instability means the model is guessing. This needs no reference at all,
and it is the only metric here that works on singing, where every other signal
is ambiguous.

## drift — how much the real-time constraints cost

The reference is the same audio transcribed in **one pass** by the strongest
config, with the whole file for context and no latency budget. That is not
ground truth, but it is a fixed, honest yardstick for the question actually
being asked: how much accuracy does chunking into 3-second pieces give away?
Every streaming config is scored against it with CER.

CER, not WER: Korean is agglutinative and its spacing is inconsistent even
between humans, so word-level scoring punishes a correct transcript for
choosing a different space.

## suspicion — known-bad shapes

Blocklist phrases, decoder loops, language flips. The same rules the app runs,
counted rather than silently applied, so a config that produces less garbage
scores better even when the garbage would have been filtered.
"""

from __future__ import annotations

import re
import unicodedata
from dataclasses import dataclass, field


# ── text normalisation ───────────────────────────────────────────────────────

_PUNCT = re.compile(r"[^\wㄱ-ㆎ가-힣一-鿿]+")


def normalise(text: str) -> str:
    """Fold away everything two transcripts may legitimately disagree on.

    Case, punctuation, whitespace, and Unicode composition. Korean survives
    round-tripping through NFC — a decomposed 한 and a composed 한 are the same
    character to a reader and must be the same character to the scorer.
    """
    t = unicodedata.normalize("NFC", text).lower()
    return _PUNCT.sub("", t)


# ── edit distance ────────────────────────────────────────────────────────────


def _levenshtein(a: str, b: str) -> int:
    if a == b:
        return 0
    if not a:
        return len(b)
    if not b:
        return len(a)
    # Two rows rather than a full matrix: these are subtitle lines, but a whole
    # transcript is passed in too and the matrix would be tens of MB.
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


def cer(hypothesis: str, reference: str) -> float:
    """Character error rate, 0.0 = identical. Can exceed 1.0 on insertions."""
    h, r = normalise(hypothesis), normalise(reference)
    if not r:
        return 0.0 if not h else 1.0
    return _levenshtein(h, r) / len(r)


# ── metric 1: instability across repeated runs ───────────────────────────────


def instability(runs: list[str]) -> float:
    """Mean pairwise CER between repeated transcriptions of one chunk.

    0.0 = every run identical (the model is sure). Toward 1.0 = the model is
    inventing something different each time.
    """
    texts = [t for t in runs if t is not None]
    if len(texts) < 2:
        return 0.0
    pairs = [
        cer(texts[i], texts[j])
        for i in range(len(texts))
        for j in range(i + 1, len(texts))
    ]
    # Symmetrised: CER is not commutative (it divides by the reference length),
    # and which run is "reference" here is arbitrary.
    back = [
        cer(texts[j], texts[i])
        for i in range(len(texts))
        for j in range(i + 1, len(texts))
    ]
    return sum(pairs + back) / len(pairs + back)


# ── metric 2: known-bad shapes ───────────────────────────────────────────────

# Mirrors DENY in src-tauri/src/asr/http_client.rs. Kept as a copy on purpose:
# the bench must be able to count a phrase the app has not learned to block
# yet, which is the whole reason the list keeps growing.
BLOCKLIST = (
    "字幕由", "請訂閱", "感謝收看", "謝謝收看", "歡迎訂閱",
    "thanks for watching", "thank you for watching", "like and subscribe",
    "please subscribe", "see you in the next video",
    "한글자막", "한효정", "다음 영상에서 만나요",
    "시청해주셔서 감사합니다", "시청해 주셔서 감사합니다", "구독과 좋아요",
    "자막 제공", "자막제공",
    "[music]", "[blank_audio]", "[applause]", "[laughter]", "[silence]",
    "[음악]", "[박수]",
)

MAX_TOKEN_RUN = 6  # mirrors http_client.rs


def longest_token_run(text: str) -> int:
    longest = run = 0
    prev = None
    for tok in text.split():
        run = run + 1 if tok == prev else 1
        longest = max(longest, run)
        prev = tok
    return longest


def repeated_phrase(text: str) -> int:
    """How many times the most-repeated sentence in `text` appears.

    `longest_token_run` counts one *token* repeating. The loops this misses
    repeat a whole clause: "2부에서 계속됩니다. 2부에서 계속됩니다." alternates two
    tokens, so the token run never exceeds 1 and the text sails through.
    """
    parts = [p.strip() for p in re.split(r"[.!?。！？]+", text) if p.strip()]
    if not parts:
        return 0
    counts = {}
    for p in parts:
        counts[p] = counts.get(p, 0) + 1
    return max(counts.values())


# Canned closing lines Korean video ASR reaches for when there is nothing to
# transcribe. Listed as a family rather than one at a time: across five
# measured sessions each blocked phrase was replaced by the next one, and the
# bench exists to count the ones the app has not met yet.
#
# Deliberately NOT copied into the app's DENY list — several are real speech in
# conversation ("감사합니다" is "thank you"). Here, where the question is "how
# often did this config produce a stock phrase over an instrumental", counting
# them is exactly right; blocking them in the app would eat real subtitles.
CANNED_KO = (
    "감사합니다", "고맙습니다", "수고하셨습니다", "수고하세요",
    "2부에서 계속", "다음 시간에", "구독", "좋아요",
)

MIN_PHRASE_REPEAT = 3  # the same sentence three times over is not speech


def suspicion(text: str) -> list[str]:
    """Names every known-bad shape `text` matches. Empty means nothing known."""
    flags = []
    t = text.strip()
    low = t.lower()
    if t.startswith("[") and t.endswith("]"):
        flags.append("bracket-tag")
    if any(b in low for b in BLOCKLIST):
        flags.append("blocklist")
    if longest_token_run(t) >= MAX_TOKEN_RUN:
        flags.append("decoder-loop")
    if repeated_phrase(t) >= MIN_PHRASE_REPEAT:
        flags.append("phrase-loop")
    # Short and nothing but a stock phrase. The length bound matters: the same
    # words inside a longer sentence are ordinary speech.
    if len(t) <= 24 and any(c in t for c in CANNED_KO):
        flags.append("canned")
    return flags


# ── aggregation ──────────────────────────────────────────────────────────────


@dataclass
class ChunkResult:
    """One chunk under one config: what came back, and how long it took."""

    index: int
    seconds: float
    runs: list[str] = field(default_factory=list)
    langs: list[str] = field(default_factory=list)
    no_speech: list[float] = field(default_factory=list)
    ms: list[float] = field(default_factory=list)

    @property
    def text(self) -> str:
        """The first run, which is what a single-shot caller would have seen."""
        return self.runs[0] if self.runs else ""


@dataclass
class ConfigScore:
    name: str
    chunks: int
    instability: float
    suspicion_rate: float
    lang_flip_rate: float
    empty_rate: float
    median_ms: float
    p90_ms: float
    drift: float | None = None       # CER against the single-pass reference
    chars: int = 0

    def row(self) -> str:
        d = "—" if self.drift is None else f"{self.drift:.3f}"
        return (
            f"| {self.name} | {d} | {self.instability:.3f} | "
            f"{self.suspicion_rate:.0%} | {self.lang_flip_rate:.0%} | "
            f"{self.empty_rate:.0%} | {self.median_ms:.0f} | {self.p90_ms:.0f} |"
        )


HEADER = (
    "| config | drift↓ | instability↓ | suspicious↓ | lang-flip↓ | empty | "
    "median ms | p90 ms |\n"
    "|---|---|---|---|---|---|---|---|"
)


def score(name: str, results: list[ChunkResult], reference: str | None) -> ConfigScore:
    import statistics

    n = max(1, len(results))
    texts = [r.text for r in results]
    all_ms = [m for r in results for m in r.ms] or [0.0]

    # The dominant language across the file, so a flip is measured against what
    # this audio actually is rather than against a hint that may be "auto".
    langs = [l for r in results for l in r.langs if l]
    dominant = max(set(langs), key=langs.count) if langs else None

    return ConfigScore(
        name=name,
        chunks=len(results),
        instability=sum(instability(r.runs) for r in results) / n,
        suspicion_rate=sum(1 for t in texts if suspicion(t)) / n,
        lang_flip_rate=(
            sum(1 for r in results if r.langs and r.langs[0] != dominant) / n
            if dominant
            else 0.0
        ),
        empty_rate=sum(1 for t in texts if not t.strip()) / n,
        median_ms=statistics.median(all_ms),
        p90_ms=sorted(all_ms)[min(len(all_ms) - 1, int(len(all_ms) * 0.9))],
        drift=None if reference is None else cer(" ".join(texts), reference),
        chars=sum(len(t) for t in texts),
    )
