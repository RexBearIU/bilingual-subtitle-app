# Architecture Decision Records

Short, dated records of non-obvious choices and *why*. Append new ones; don't
rewrite history — supersede instead.

---

## ADR-0001 — Sidecar-first for native engines (whisper.cpp / llama.cpp)

**Date:** 2026-06-06 · **Status:** Accepted

**Context.** whisper.cpp and llama.cpp are C/C++. Two integration paths: (a) Rust
FFI / native linking, (b) run their bundled HTTP servers as Tauri sidecars.
Building from source on Windows needs CMake + a C++ toolchain and is the most
likely place to lose days.

**Decision.**
- **Translation (Qwen): `llama-server` sidecar, permanently.** It exposes an
  OpenAI-compatible HTTP API, keeps the model + KV cache resident, supports GPU
  builds, and translation payloads are tiny strings → HTTP cost is negligible.
  This is also the long-term answer, not a stepping stone.
- **ASR (whisper): `whisper-server` sidecar for v1**, to avoid the native build
  toolchain and ship fast using official prebuilt Windows binaries.

**Consequence / known trade-off.** ASR chunks are 16 kHz PCM arrays. Over HTTP
they must be serialized (WAV) per request, adding per-chunk overhead. If that
latency becomes the bottleneck, migrate **only ASR** to in-process
[`whisper-rs`](https://crates.io/crates/whisper-rs) FFI (audio buffers passed
directly, no serialization). Change one engine at a time; never both at once.

---

## ADR-0002 — WASAPI loopback, not cpal

**Date:** 2026-06-06 · **Status:** Accepted

**Context.** The spec suggested cpal for cross-platform audio. cpal's Windows
loopback support has historically been weak/unstable, and this app is Windows-only.

**Decision.** Capture the default render endpoint in loopback mode using the
[`wasapi`](https://crates.io/crates/wasapi) crate (or `windows-rs` directly with
`AUDCLNT_STREAMFLAGS_LOOPBACK`). No cpal.

**Consequence.** Capture code is Windows-specific by design. Acceptable — the
overlay and loopback are both inherently Windows-native (see ADR-0003).

---

## ADR-0003 — Windows-native only; no WSL

**Date:** 2026-06-06 · **Status:** Accepted

WASAPI loopback and the transparent always-on-top overlay both require a native
Windows host. All build/run happens on Windows. WSL is explicitly unsupported.

---

## ADR-0004 — Frontend: Svelte + Vite (no SvelteKit)

**Date:** 2026-06-06 · **Status:** Accepted

The app is a single transparent overlay with no routing, no SSR, no server. Plain
Svelte + Vite is lighter and sufficient. Revisit only if a multi-page settings
surface justifies routing.

---

## ADR-0005 — Dev injection instead of a "mock" stage

**Date:** 2026-06-06 · **Status:** Accepted

**Context.** The overlay (M1) must be testable before audio/ASR (M2/M4) exist, but
the user wants real implementation, not throwaway mock code.

**Decision.** No fake-subtitle product feature. Instead a dev-only command
`dev_inject_subtitle` emits a **real** `subtitle_update` through the **real** event
path — only the data source is manual during early milestones. When M4 lands, real
ASR output flows through the identical path; the dev command is feature-gated out
of release builds. Nothing gets thrown away.

---

## ADR-0006 — faster-whisper over whisper.cpp server

**Date:** 2026-06-08 · **Status:** Accepted · **Supersedes:** ADR-0001 (ASR half only)

**Context.** The original ADR-0001 used the official `whisper-server.exe` prebuilt
from whisper.cpp.  Two issues surfaced: (1) the C++ binary did not return
`no_speech_prob` in its `verbose_json` output, making hallucination filtering
unreliable; (2) model variants (large-v3-turbo, distil-whisper) were not available
as prebuilt Windows executables, limiting upgrade paths.

**Decision.** Switch the ASR sidecar to `faster_whisper_srv.py` — a Python
`fastapi` server wrapping the `faster-whisper` library (CTranslate2 backend).
- Returns full `verbose_json` including `no_speech_prob` per segment → reliable
  silence/noise suppression.
- Models are downloaded automatically on first run from HuggingFace (no manual
  binary management).
- GPU acceleration via CTranslate2 (CUDA or CPU fallback).

**Consequence.** Requires Python 3.10+ and `pip install faster-whisper fastapi
uvicorn ctranslate2` on the target machine. The HTTP API is the same multipart
`/inference` endpoint, so `asr/whisper_server.rs` needed only minor changes (longer
startup timeout for first-run model download, `no_speech_prob` extraction). The
`WHISPER_SERVER_BIN` env var is replaced by `PYTHON_BIN` + `WHISPER_SERVER_SCRIPT`;
`WHISPER_MODEL` now accepts a HuggingFace repo ID or local directory path.

---

## ADR-0007 — SubtitleMode redesign: single-language target

**Date:** 2026-06-08 · **Status:** Accepted

**Context.** The original design used `zh-ko` / `zh-en` bilingual modes where
two languages were always shown. This created ambiguity in the translation pipeline:
the target language depended on the detected source language (e.g. zh-ko showed
Korean for Chinese input, Chinese for Korean input). This complexity was error-prone
and made it impossible to add "translate everything to Chinese regardless of source".

**Decision.** Replace the bilingual modes with a **single-target** model:
- `"zh"` — translate to Traditional Chinese (繁體中文)
- `"ko"` — translate to Korean (한국어)
- `"en"` — translate to English
- `"none"` — source text only, no translation

The target is always fixed. The source language is whatever Whisper detects. If the
source already matches the target, the translation step emits the source text directly.

**Consequence.** `SubtitleTexts` can hold all three language slots, but only the
source + target slots are populated per event. Frontend renders whatever is present.
`SourceHint` (ADR-0007b) is added separately to let users lock Whisper's detection
for single-language streams.

---

## ADR-0007b — SourceHint for Whisper language lock

**Date:** 2026-06-08 · **Status:** Accepted

**Context.** In a monolingual stream (e.g. a Korean YouTube video), Whisper's
per-chunk auto-detection occasionally misclassifies a chunk as Chinese or English,
producing garbage. Users want to lock detection without changing the translation target.

**Decision.** Add `SourceHint { Auto, Zh, Ko, En }` as a separate control. When
set to a specific code, the `language` field is sent to Whisper per request. `Auto`
(default) retains per-chunk detection.  Persisted in settings; hot-swappable.

---

## ADR-0008 — Per-process audio capture

**Date:** 2026-06-08 · **Status:** Accepted

**Context.** System-wide WASAPI loopback captures all audio — background music,
notification sounds, etc. — causing spurious subtitles. Gaming users in particular
want to caption only game dialogue or only a specific streaming app.

**Decision.** Expose Windows Process Loopback API (`ActivateAudioInterfaceAsync`
+ `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`) via `audio/process_loopback.rs`.
A `list_audio_processes` command enumerates active audio sessions; `set_capture_process`
lets the frontend target a specific PID. `pid: 0` reverts to system-wide loopback.
Requires Windows 10 Build 20348 / Windows 11.

**Consequence.** The capture path bifurcates in `audio/capture.rs` — system loopback
vs process loopback. Change takes effect on next `start_captioning` (no hot-swap
while running). COM must be initialised as MTA on a fresh thread for `list_audio_processes`
(Tauri's WebView2 STA thread is incompatible).

---

## ADR-0009 — Replace RMS VAD with fixed-chunk accumulator

**Date:** 2026-06-08 · **Status:** Accepted · **Supersedes:** M3 VAD design

**Context.** The original `pipeline/vad.rs` used an adaptive RMS threshold with
exponential noise-floor EMA and onset/silence state machine. This worked for
microphone input but proved unreliable for the primary use case — video and live
stream loopback — for two reasons:
1. Background music keeps the RMS continuously above any sensible speech threshold,
   so either speech is missed (threshold too high) or silence chunks flood ASR
   (threshold too low). No single threshold works across content types.
2. Whisper already provides `no_speech_prob` per segment, which is a superior
   silence discriminator trained on diverse audio — no RMS tuning required.

**Decision.** Delete `pipeline/vad.rs` (and `audio/ring_buffer.rs` which only it
used). Replace with `pipeline/chunker.rs`: a simple fixed-chunk accumulator that
emits 4 s chunks (64 000 samples @ 16 kHz) unconditionally. Music mode still uses
10 s chunks. Silence detection is fully delegated to Whisper's `no_speech_prob ≥ 0.7`
filter in the ASR worker.

**Consequence.** The pipeline thread structure is unchanged — a chunker worker is
still needed to buffer the 200 ms capture events into ASR-sized chunks without
blocking the capture thread. `speech_threshold` is retained in settings/state/IPC
for API compatibility but is no longer read by the chunker. Whisper may process a
slightly higher volume of chunks (a silent 4 s chunk every 4 s instead of nothing)
but `no_speech_prob` drops them cheaply before any translation is attempted.
