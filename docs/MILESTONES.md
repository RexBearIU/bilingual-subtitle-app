# Milestones

Status legend: ⬜ not started · 🟡 in progress · ✅ done

| # | Milestone | Status |
|---|-----------|--------|
| 0 | Repo scaffold, docs, toolchain | ✅ |
| 1 | Tauri overlay shell | ✅ |
| 2 | WASAPI system audio capture | ✅ |
| 3 | Audio chunking + VAD | ✅ |
| 4 | Local ASR (whisper.cpp) | ✅ |
| 5 | Translation engine (Qwen) | ⬜ |
| 6 | Subtitle state manager | ⬜ |
| 7 | Product settings | ⬜ |
| 8 | Performance optimization | ⬜ |
| 9 | Optional SenseVoice backend | ⬜ |

---

## M0 — Scaffold & toolchain  ✅

- [x] Decide stack & record ADRs ([DECISIONS.md](DECISIONS.md))
- [x] Write planning docs
- [x] Install Rust (rustup 1.29 / rustc 1.96 msvc) + Node LTS (24.16)
- [x] Scaffold Tauri v2 + Svelte + Vite (TS) project
- [x] `cargo check` clean · `npm run check` clean
- [x] `git init` on `main`, `.gitignore` (models/ + binaries/ excluded)

## M1 — Tauri overlay shell  ✅

**Build the real event pipeline end-to-end with a dev injection source (ADR-0005).**

Frontend (`src/`):
- [x] Transparent, frameless, always-on-top window (`tauri.conf.json`)
- [x] Click-through toggle (`set_ignore_cursor_events`)
- [x] Draggable subtitle area (`data-tauri-drag-region`)
- [x] Font-size setting (slider → `set_font_size`)
- [x] Subtitle-mode setting (zh-ko / zh-en segmented control)
- [x] Start / stop button
- [x] Model/engine status display (status dots)
- [x] **Visual verification via `npm run tauri dev`** — transparent ✓, draggable ✓,
      mode switch ✓, inject ✓, on-top toggle ✓, click-through + recovery ✓
- [x] System tray (checkable 穿透/置頂 + 結束) and `Ctrl+Alt+P` escape hatch
- [x] Click-through hides control bar (clean caption-only overlay)

Backend commands (`src-tauri/src/commands.rs`): `start_captioning`,
`stop_captioning`, `set_subtitle_mode`, `set_click_through`, `set_font_size`,
`get_status`, `dev_inject_subtitle`. Event: `subtitle_update` + `engine_status`
(see [IPC-CONTRACT.md](IPC-CONTRACT.md)).

> **Click-through lockout — solved two ways:** enabling click-through makes the
> whole window pass-through, so no in-overlay button is clickable. Recovery is
> therefore handled *outside* the overlay:
> 1. **System tray icon** (`TrayIconBuilder` in `lib.rs`) — always clickable.
>    Menu: 停用穿透 / 切換置頂 / 結束. This is the primary "escape button".
> 2. **Global hotkey `Ctrl+Alt+P`** (`tauri-plugin-global-shortcut`) — backup.
>
> Both call the shared `force_interactive()` helper. Also: `set_always_on_top`
> command + 📌 toggle button (pin/unpin) in the control bar.

**Acceptance:** opens as transparent overlay · displays injected (real-path)
subtitles · zh-ko/zh-en switch works · stays above browser/video.

## M2 — WASAPI capture  ✅

Modules: `audio/{mod,capture,resample,ring_buffer,meter}.rs`.
**Acceptance:** YouTube playback → non-zero captured audio · RMS shown in
debug/UI · no mic · no WSL.

**Verified:** WASAPI loopback stream at 192 kHz / 2 ch / 32 bps f32.
Start→stop lifecycle clean. RMS emitted to frontend via `engine_status`.
Tauri hot-rebuild round-trip 6.6 s.

## M3 — Chunking + VAD  ✅

RMS VAD v1. 16kHz mono · chunk 2–5s · pre-roll ~300ms · silence timeout
500–800ms · max segment 8s · configurable threshold.
**Acceptance:** silence doesn't trigger ASR · speech produces chunks · chunk
start/end timestamps logged. Later: Silero/WebRTC VAD.

**Implemented:** `pipeline/vad.rs` — RMS VAD state machine (25 ms frames,
SPEECH_THRESHOLD=0.005 ≈ −46 dBFS, 300 ms pre-roll, 500 ms silence timeout,
8 s max). `capture.rs` resamples WASAPI output to 16 kHz mono via
`audio/resample.rs` (rubato SincFixedIn) and forwards to VAD via `mpsc`
channel. `pipeline/mod.rs` declared; `lib.rs` includes `mod pipeline`.

## M4 — ASR (whisper.cpp)  ✅

`whisper-server` sidecar (ADR-0001). Model `ggml-medium.bin`, later
`large-v3-turbo`. Load once. Return text + detected lang (ko/en/zh) + timestamps.
Keep prior context/prompt for continuity.
**Acceptance:** ko/en/zh transcribed · lang auto-detect · source subtitle emitted
without translation.

**Implemented:** `asr/mod.rs` (`AudioChunk` type) + `asr/whisper_server.rs`
(HTTP client with multipart WAV upload, verbose_json response parsing,
`normalize_lang` mapping, `encode_wav_16bit`, `subtitle_update` emission).
`commands.rs` launches `whisper-server` via `std::process::Command` on
`start_captioning` (env-configurable: `WHISPER_SERVER_BIN`, `WHISPER_MODEL`,
`WHISPER_ASR_PORT`; defaults: PATH lookup, `models/ggml-medium.bin`, 9001).
`state.rs` adds `asr_status` + `WhisperProc` managed state. ASR worker polls
for server readiness (30 s), then streams chunks from VAD → WAV → POST →
`subtitle_update`. `ureq` v2 used for synchronous HTTP (no tokio conflict).

**To activate:** place `whisper-server.exe` on PATH (or set `WHISPER_SERVER_BIN`)
and put a model at `models/ggml-medium.bin` (or set `WHISPER_MODEL`) relative
to the working directory when running `npm run tauri dev`.

## M5 — Translation (Qwen)  ⬜

`llama-server` sidecar. Models: Qwen2.5-1.5B-Instruct Q4_K_M; upgrade to Qwen3-4B
if quality insufficient. Output subtitle text only · Traditional Chinese · natural
subtitle style · preserve names/brands/common English tech terms · no explanations.
Mode logic in [ARCHITECTURE.md](ARCHITECTURE.md).
**Acceptance:** ko→zh-en · en→zh-ko · zh→zh-en/zh-ko · acceptable latency.

## M6 — Subtitle state manager  ⬜

`SubtitleSegment` store: dedup · merge fragments · expire after 3–5s · partial &
final · keep last N segments as translation context.
**Acceptance:** no flicker · no duplicate text · subtitles disappear naturally.

## M7 — Settings  ⬜

mode · ASR model path · translation model path · font size · max lines · overlay
position · opacity · click-through · low-latency / high-quality. Persisted via
`tauri-plugin-store`.
**Acceptance:** settings survive restart · mode changeable while running.

## M8 — Performance  ⬜

Targets: 1–3s end-to-end · low idle CPU · models stay loaded · no memory growth.
Separate worker threads + bounded channels · drop stale chunks under back-pressure.

## M9 — SenseVoice (optional)  ⬜

Add as alternative ASR backend behind `AsrBackend` trait. Settings toggle
whisper.cpp / SenseVoice. Same downstream pipeline.
