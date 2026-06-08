# Milestones

Status legend: ⬜ not started · 🟡 in progress · ✅ done

| # | Milestone | Status |
|---|-----------|--------|
| 0 | Repo scaffold, docs, toolchain | ✅ |
| 1 | Tauri overlay shell | ✅ |
| 2 | WASAPI system audio capture | ✅ |
| 3 | Audio chunking + VAD | ✅ |
| 4 | Local ASR (whisper.cpp) | ✅ |
| 5 | Translation engine (Qwen) | ✅ |
| 6 | Subtitle state manager | ✅ |
| 7 | Product settings | ✅ |
| 8 | Performance optimization | ✅ |
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
`commands.rs` launches `faster_whisper_srv.py` (Python) via `std::process::Command`
on `start_captioning` (env-configurable: `PYTHON_BIN`, `WHISPER_SERVER_SCRIPT`,
`WHISPER_MODEL` as HuggingFace repo ID, `WHISPER_ASR_PORT`; defaults: `python`,
`faster_whisper_srv.py`, `Systran/faster-whisper-medium`, 9001).
`state.rs` adds `asr_status` + `WhisperProc` managed state. ASR worker polls
for server readiness (300 s — allows first-run model download), then streams
chunks from VAD → WAV → POST → `subtitle_update`. `ureq` v2 used for synchronous
HTTP (no tokio conflict).

**ASR quality filters** (added post-M4, in `whisper_server.rs`):
- `no_speech_prob ≥ 0.7` suppresses silence/noise chunks
- Hallucination blocklist (YouTube credits, `[Music]`, etc.)
- Consecutive-repeat detection (initial_prompt feedback loop guard)

**To activate:** place `whisper-server.exe` on PATH (or set `WHISPER_SERVER_BIN`)
and put a model at `models/ggml-medium.bin` (or set `WHISPER_MODEL`) relative
to the working directory when running `npm run tauri dev`.

## M5 — Translation (Qwen)  ✅

`llama-server` sidecar. Models: Qwen2.5-1.5B-Instruct Q4_K_M; upgrade to Qwen3-4B
if quality insufficient. Output subtitle text only · Traditional Chinese · natural
subtitle style · preserve names/brands/common English tech terms · no explanations.
Mode logic in [ARCHITECTURE.md](ARCHITECTURE.md).
**Acceptance:** ko→zh-en · en→zh-ko · zh→zh-en/zh-ko · acceptable latency.

**Implemented:** `translate/mod.rs` (`TranslationRequest` boundary type) +
`translate/llama_server.rs` (HTTP client calling OpenAI-compatible
`/v1/chat/completions`). Qwen3 `/no_think` directive used to suppress
chain-of-thought and get direct translation output. `strip_think_tags` safety-net
strips any residual `<think>…</think>` blocks. `state.rs` adds `translation_status`
+ `LlamaProc` managed state. `commands.rs` launches `llama-server` on
`start_captioning` (env-configurable: `LLAMA_SERVER_BIN`, `LLAMA_MODEL`,
`LLAMA_PORT`, `LLAMA_GPU_LAYERS`; defaults: PATH, `models/Qwen3-4B-Q4_K_M.gguf`,
9002, 36 GPU layers). ASR worker emits source-only subtitle immediately
(`is_final=false`), then enqueues a `TranslationRequest`; translation worker emits
updated event with same `id` and `zh` slot filled (`is_final=true`). Pipeline:
WASAPI → VAD → ASR → [translate channel] → Translation → `subtitle_update`.

## M6 — Subtitle state manager  ✅

`SubtitleSegment` store: dedup · merge fragments · expire after 3–5s · partial &
final · keep last N segments as translation context.
**Acceptance:** no flicker · no duplicate text · subtitles disappear naturally.

**Implemented:** `subtitles.svelte.ts` — `OverlayStore.segments: SubtitleUpdate[]`
(Svelte 5 `$state`). `_handleUpdate()`: dedup by `id` (splice-replace in-place) +
merge partial→final (same slot). `_expiry: Map<id, number|null>` tracks
per-segment expiry timestamp (set when `is_final=true`, null while still partial).
`_prune()` runs every 500ms via `setInterval`, removes segments past their expiry.
`MAX_SEGMENTS=3` caps the display count; oldest are evicted when over limit.
`SubtitleView.svelte` now accepts `segments: SubtitleUpdate[]` and stacks them
oldest-first; partial segments get `opacity: 0.75` while awaiting translation.
`EXPIRE_MS=4000` (4s on-screen after final).

## M7 — Settings  ✅

mode · ASR model path · translation model path · font size · max lines · overlay
position · opacity · click-through · low-latency / high-quality. Persisted via
`tauri-plugin-store`.
**Acceptance:** settings survive restart · mode changeable while running.

**Implemented:** `settings.rs` — `PersistSettings` + `OverlayRect` structs,
JSON file stored at `{AppData}/com.bilingualsubtitle.app/settings.json` (no
plugin dependency, uses `serde_json` + `std::fs`). `SettingsPath` managed state
holds the resolved path.  `setup` in `lib.rs` loads settings at launch, applies
window position/size via `set_position`/`set_size`, syncs AppState
(mode/font_size/subtitle_opacity/llama_gpu_layers). Commands: `get_settings`
(returns current settings) and `update_settings(patch)` (partial update →
AppState + file). `set_font_size` / `set_subtitle_mode` call `save_current_settings`
after updating.  Frontend: `App.svelte` listens to `window.onMoved` /
`window.onResized` (400ms debounce) → `updateSettings({overlay})`.  ControlBar
adds opacity slider (◐ icon) and GPU/CPU toggle button (persists
`llama_gpu_layers`: 36 ↔ 0). Subtitle background uses CSS `--subtitle-bg-opacity`
custom property driven by `EngineStatus.subtitleOpacity`.

## M8 — Performance  ✅

Targets: 1–3s end-to-end · low idle CPU · models stay loaded · no memory growth.
Separate worker threads + bounded channels · drop stale chunks under back-pressure.

**Implemented:**
- **Bounded channels** — VAD→ASR and ASR→Translation channels changed from
  `mpsc::channel` (unbounded) to `mpsc::sync_channel(2)`. VAD and ASR use
  `try_send`; if the consumer is busy and the queue is full, the chunk is dropped
  with a WARN log instead of piling up in memory. Capture→VAD stays unbounded
  (VAD is fast — pure RMS arithmetic).
- **Whisper rolling prompt** — after each successful transcription, the last
  ≤200 chars of text are passed as `initial_prompt` to the next request.
  This improves continuity (names, punctuation, sentence context) across chunk
  boundaries at zero latency cost.
- **RMS log → debug** — audio meter was logging at INFO every 200 ms (5 lines/s).
  Changed to DEBUG to keep the log readable during normal use.
- **Adaptive VAD** — `speech_threshold == 0` activates noise-floor EMA auto-mode.
  3-frame onset detection (75 ms) suppresses music beats and game SFX. Partial
  flush every 5 s keeps subtitles appearing without waiting for silence.
- **Music mode** — bypasses VAD; fixed 10 s chunks + "Song lyrics:" prompt;
  beam_size=3 for better lyric accuracy.
- **Per-process capture** — `audio/process_loopback.rs` uses Windows Process
  Loopback API to capture a single PID. `list_audio_processes` / `set_capture_process`
  commands. `audio/session_enum.rs` for audio session enumeration.
- **SubtitleMode redesign** — `zh-ko`/`zh-en` bilingual modes replaced by single-
  target `none`/`zh`/`ko`/`en` (ADR-0007). `SourceHint` added for Whisper language lock.
- **faster-whisper ASR** — switched from whisper.cpp binary to Python faster-whisper
  sidecar for `no_speech_prob` access and easier model management (ADR-0006).

## M9 — SenseVoice (optional)  ⬜

Add as alternative ASR backend behind `AsrBackend` trait. Settings toggle
whisper.cpp / SenseVoice. Same downstream pipeline.
