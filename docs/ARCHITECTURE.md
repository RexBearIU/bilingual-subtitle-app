# Architecture

## Pipeline

```text
Windows default output device (or specific process via Process Loopback API)
        │  WASAPI loopback / Process Loopback (shared mode)
        ▼
[capture thread]  f32 interleaved @ device rate / channels
        │  → ring buffer (bounded)
        ▼
[resample]  → 16 kHz mono f32
        ▼
[VAD / chunking worker]  adaptive RMS VAD (or music-mode fixed 10 s chunks)
        │  emits speech segments (pre-roll 300 ms, partial flush every 5 s, max 12 s)
        ▼
[ASR worker]  faster-whisper (Python HTTP sidecar)
        │  → { text, lang(ko|en|zh), no_speech_prob }
        │  filters: no_speech_prob ≥ 0.7, hallucination blocklist, consecutive-repeat
        ▼
[Translation worker]  llama-server (Qwen3-4B, Vulkan GPU)
        │  → target-language subtitle text (single language per mode)
        ▼
[Subtitle state manager]  dedup by id / partial→final merge / expiry pruning
        │
        ▼
Tauri event  `subtitle_update`
        ▼
Svelte transparent overlay (render-only)
```

## Thread / channel model

Each stage is its own worker, connected by **bounded channels**. Back-pressure
policy: **drop stale chunks** rather than block, so realtime latency is preserved
when a stage falls behind.

```text
capture(thread) ─► ring buffer ─► vad(worker) ─►[seg sync_channel(2)]─► asr(worker)
   ─►[asr sync_channel(2)]─► translate(worker) ─► state mgr ─► emit
```

- Models (whisper, llama) are **loaded once** and kept resident for the whole
  session. Sidecars stay alive across Stop/Start cycles — no model reload.
- The audio ring buffer is **bounded**; oldest samples are overwritten.
- No unbounded queues anywhere → no memory growth over long sessions.

## Process topology

```text
Tauri app (Rust)
 ├─ owns: WASAPI/process capture, ring buffer, VAD, state manager, UI events
 ├─ spawns: python faster_whisper_srv.py  (HTTP :9001)  ── ASR
 └─ spawns: llama-server.exe             (HTTP :9002)  ── translation (Vulkan GPU)
```

Sidecars are launched on the first `start_captioning` call and stay alive until
the app exits (Drop impl sends SIGKILL). See [DECISIONS.md](DECISIONS.md) ADR-0001,
ADR-0006 for sidecar rationale and the faster-whisper choice.

## Subtitle mode logic

`mode` selects the **target translation language**. The source language detected by
ASR is shown in its original slot; the target language is translated by Qwen.

```text
mode = "zh"   → translate source (ko/en/ja/…) to Traditional Chinese
mode = "ko"   → translate source to Korean
mode = "en"   → translate source to English
mode = "none" → source text only, no translation call
```

If the detected source language already matches the target, the translation worker
emits the source text as-is (no LLM call needed).

## Adaptive VAD

When `speech_threshold == 0` (default), the VAD maintains an exponential moving
average (EMA) of quiet-frame RMS and gates at `noise_ema × 4` (clamped
0.003–0.12). Music beats and game SFX are suppressed by requiring 3 consecutive
speech frames (75 ms onset) before an utterance opens.

Music mode bypasses VAD entirely: audio is sent in fixed 10 s chunks with a
"Song lyrics:" prompt prepended to improve lyric accuracy.

## Per-process capture

When a `capture_target` is set (via `set_capture_process`), the Rust backend uses
the Windows **Process Loopback API** (`ActivateAudioInterfaceAsync` +
`AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`) to capture audio from only that
process tree. Falls back to system-wide WASAPI loopback when no target is set.
Requires Windows 10 Build 20348 or Windows 11.

## Hallucination filtering

The ASR worker applies two layers before emitting any subtitle:

1. **`no_speech_prob` filter** — faster-whisper returns a per-segment probability
   that the audio contains no speech. Segments with mean ≥ 0.7 are dropped.
2. **Blocklist filter** — known hallucination phrases (YouTube credits, `[Music]`,
   `[BLANK_AUDIO]`, etc.) are blocked by substring match.
3. **Consecutive-repeat filter** — exact repetition of the last valid segment
   (for short texts < 60 chars) indicates a `initial_prompt` feedback loop.

## Backend module layout

```text
src-tauri/src/
├─ main.rs                    # Tauri builder entry point
├─ lib.rs                     # setup: managed state, tray, shortcuts, commands
├─ commands.rs                # #[tauri::command] handlers; sidecar launch helpers
├─ state.rs                   # AppState + WhisperProc + LlamaProc managed state
├─ types.rs                   # Shared IPC types (SubtitleMode, EngineStatus, …)
├─ settings.rs                # PersistSettings + OverlayRect; JSON file I/O
├─ pipeline/
│  ├─ mod.rs                  # orchestration: channel wiring, start/stop
│  └─ vad.rs                  # adaptive RMS VAD + chunking + music mode
├─ audio/
│  ├─ mod.rs
│  ├─ capture.rs              # WASAPI system-wide loopback (wasapi crate)
│  ├─ process_loopback.rs     # Per-process loopback (Windows Process Loopback API)
│  ├─ session_enum.rs         # List active audio sessions (for process picker)
│  ├─ resample.rs             # → 16 kHz mono (rubato SincFixedIn)
│  ├─ ring_buffer.rs
│  └─ meter.rs                # RMS debug meter
├─ asr/
│  ├─ mod.rs                  # AudioChunk type
│  └─ whisper_server.rs       # faster-whisper HTTP client; hallucination filters
└─ translate/
   ├─ mod.rs                  # TranslationRequest type
   └─ llama_server.rs         # llama-server HTTP client (OpenAI-compatible)
```

## Frontend layout

```text
src/
├─ App.svelte                 # overlay root (transparent, render-only)
├─ main.ts
├─ lib/
│  ├─ subtitles.svelte.ts     # OverlayStore: subscribe to events, hold render state
│  ├─ commands.ts             # typed wrappers over invoke()
│  └─ types.ts                # IPC types (mirrors src-tauri/src/types.rs)
└─ components/
   ├─ SubtitleView.svelte     # stacked bilingual subtitle display
   ├─ ControlBar.svelte       # start/stop, mode, status, settings trigger
   ├─ ProcessPicker.svelte    # per-process audio capture selector
   └─ SettingsPanel.svelte    # settings overlay (opacity, VAD threshold, GPU layers)
```
