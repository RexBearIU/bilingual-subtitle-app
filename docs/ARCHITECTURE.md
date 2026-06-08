# Architecture

## Pipeline

```text
Windows default output device (or specific process via Process Loopback API)
        │  WASAPI loopback / Process Loopback (shared mode)
        ▼
[capture thread]  f32 interleaved @ device rate / channels
        ▼
[resample]  → 16 kHz mono f32
        ▼
[chunker worker]  fixed-size accumulator (4 s video / 10 s music mode)
        │  emits complete chunks; no VAD gating
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
capture(thread) ──► chunker(worker) ──►[sync_channel(4)]──► asr(worker)
   ──►[sync_channel(2)]──► translate(worker) ──► state mgr ──► emit
```

- Models (whisper, llama) are **loaded once** and kept resident for the whole
  session. Sidecars stay alive across Stop/Start cycles — no model reload.
- Capture→chunker is an unbounded `mpsc::channel` (chunker is fast — pure accumulation).
- Chunker→ASR is `sync_channel(4)` — drops audio chunks if ASR is saturated.
- ASR→translate is `sync_channel(2)` — drops translation requests if LLM is busy.
- No unbounded queues anywhere → no memory growth over long sessions.

## Process topology

```text
Tauri app (Rust)
 ├─ owns: WASAPI/process capture, chunker, state manager, UI events
 ├─ spawns: python faster_whisper_srv.py  (HTTP :9001)  ── ASR
 └─ spawns: llama-server.exe             (HTTP :9002)  ── translation (Vulkan GPU)
```

Sidecars are launched on the first `start_captioning` call and stay alive until
the app exits (Drop impl sends SIGKILL). `kill_port()` in `commands.rs` evicts any
zombie sidecar from a previous session before each launch. See [DECISIONS.md](DECISIONS.md)
ADR-0001, ADR-0006 for sidecar rationale and the faster-whisper choice.

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

## Fixed-chunk audio pipeline

The chunker (`pipeline/chunker.rs`) accumulates resampled 16 kHz mono samples and
emits two-phase chunks per utterance — no RMS-based VAD gating.

**Video / stream mode (default):**
1. **Partial flush** — after 1 s (16 000 samples) a partial chunk (`is_partial=true`,
   beam_size=1) is sent immediately. ASR starts while the speaker is still talking,
   showing a preliminary subtitle ~1.5 s from speech start.
2. **Final flush** — triggered by silence (≥ 400 ms of RMS < 0.005) or the 4 s cap.
   Sends remaining samples (`is_partial=false`, beam_size=5). ASR updates the
   subtitle with the accurate result and triggers translation.

**Music mode:** fixed 10 s chunks, no partial flush, beam_size=3 + "Song lyrics:" prompt.

**Silence detection:** RMS below `SILENCE_RMS = 0.005` (≈ −46 dBFS) for
`SILENCE_FRAMES = 2` consecutive ~200 ms input blocks. Falls back to 4 s cap
when background music prevents the threshold from being reached.

**Whisper-level silence filter:** chunks where `no_speech_prob ≥ 0.7` are dropped
by the ASR worker — more reliable than RMS for video/stream content (see ADR-0009).

A stop-flush sends any accumulator ≥ 0.5 s to ASR when `stop_captioning` is called.

The `speech_threshold` setting is retained in `AppState` / `PersistSettings` /
`EngineStatus` for API compatibility but is no longer read by the chunker.

## ASR worker

Uses `Systran/faster-whisper-large-v3-turbo` by default (configurable via
`WHISPER_MODEL` env var). Runs on CUDA float16 when a GPU is detected.

**beam_size strategy:**
- Partial chunks → beam_size=1 (greedy, fast preview)
- Final chunks (video) → beam_size=5 (accurate)
- Music mode → beam_size=3

**Consecutive-repeat filter:** suppresses exact repetition of the previous valid
segment (short texts < 60 chars) to prevent `initial_prompt` feedback loops.
Exception: a final chunk that completes a partial of the same utterance is exempt
— the partial and final naturally produce the same text for short sentences.

## Translation worker

Uses Qwen3-4B via `llama-server` (Vulkan GPU). Prompt is language-pair aware:
Korean source adds brief rules (keep English loanwords, phonetic name
transliteration, match formal/casual register). All modes use `/no_think` to
suppress chain-of-thought and output the translation directly.

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
├─ lib.rs                     # setup: managed state, tray, shortcuts, log filters
├─ commands.rs                # #[tauri::command] handlers; sidecar launch + kill_port()
├─ state.rs                   # AppState + WhisperProc + LlamaProc managed state
├─ types.rs                   # Shared IPC types (SubtitleMode, EngineStatus, …)
├─ settings.rs                # PersistSettings + OverlayRect; JSON file I/O
├─ pipeline/
│  ├─ mod.rs                  # pub mod chunker
│  └─ chunker.rs              # Fixed-chunk accumulator (4 s video / 10 s music)
├─ audio/
│  ├─ mod.rs
│  ├─ capture.rs              # WASAPI system-wide loopback (wasapi crate)
│  ├─ process_loopback.rs     # Per-process loopback (Windows Process Loopback API)
│  ├─ session_enum.rs         # List active audio sessions (for process picker)
│  ├─ resample.rs             # → 16 kHz mono (rubato SincFixedIn)
│  └─ meter.rs                # RMS helper (used for UI level meter)
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
   └─ SettingsPanel.svelte    # settings overlay (opacity, GPU layers)
```
