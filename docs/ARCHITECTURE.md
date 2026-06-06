# Architecture

## Pipeline

```text
Windows default output device
        │  WASAPI loopback (shared mode)
        ▼
[capture thread]  f32 interleaved @ device rate/channels
        │  → ring buffer (bounded)
        ▼
[resample]  → 16 kHz mono f32
        ▼
[VAD / chunking worker]  RMS VAD v1 → Silero/WebRTC later
        │  emits speech segments (pre-roll ~300ms, max 8s)
        ▼
[ASR worker]  whisper.cpp  → { text, lang(ko|en|zh), [timestamps] }
        │
        ▼
[Translation worker]  llama-server (Qwen)  → target-language subtitle text
        │
        ▼
[Subtitle state manager]  dedup / merge / expire / partial→final
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
capture(thread) ─► ring buffer ─► vad(worker) ─► [seg chan] ─► asr(worker)
   ─► [asr chan] ─► translate(worker) ─► [sub chan] ─► state mgr ─► emit
```

- Models (whisper, llama) are **loaded once** and kept resident for the whole
  session. Never reload per chunk.
- The audio ring buffer is **bounded**; oldest samples are overwritten.
- No unbounded queues anywhere → no memory growth over long sessions.

## Process topology (sidecar-first)

```text
Tauri app (Rust)
 ├─ owns: WASAPI capture, ring buffer, VAD, state manager, UI events
 ├─ spawns: llama-server.exe   (HTTP :PORT, OpenAI-compatible)  ── translation
 └─ spawns: whisper-server.exe (HTTP :PORT)                     ── ASR  [v1]
```

Sidecars are managed by Tauri (start on app launch, killed on exit). See
[DECISIONS.md](DECISIONS.md) ADR-0001 for why sidecar-first, and the planned
migration of ASR to in-process `whisper-rs` FFI.

## Subtitle mode logic

`mode` selects the two languages shown; the **source language** (from ASR) decides
which is original vs translated.

```text
mode = zh-ko:
  src zh → zh original + ko translation
  src ko → ko original + zh translation
  src en → zh translation + ko translation

mode = zh-en:
  src zh → zh original + en translation
  src en → en original + zh translation
  src ko → zh translation + en translation
```

## Backend module layout (target)

```text
src-tauri/src/
├─ main.rs                 # Tauri builder, command/event wiring, sidecar lifecycle
├─ commands.rs             # #[tauri::command] handlers
├─ pipeline/
│  ├─ mod.rs               # orchestration, channel wiring, start/stop
│  ├─ vad.rs               # RMS VAD + chunking
│  └─ state.rs             # SubtitleSegment store: dedup / merge / expire
├─ audio/
│  ├─ mod.rs
│  ├─ capture.rs           # WASAPI loopback (wasapi crate)
│  ├─ resample.rs          # → 16kHz mono
│  ├─ ring_buffer.rs
│  └─ meter.rs             # RMS debug meter
├─ asr/
│  ├─ mod.rs               # trait AsrBackend
│  └─ whisper_server.rs    # sidecar HTTP client  [v1]
├─ translate/
│  ├─ mod.rs               # trait Translator
│  └─ llama_server.rs      # llama-server HTTP client
└─ settings.rs             # persisted config (tauri-plugin-store)
```

## Frontend layout (target)

```text
src/
├─ App.svelte              # overlay root (transparent, render-only)
├─ lib/
│  ├─ subtitles.ts         # subscribe to `subtitle_update`, hold render state
│  ├─ commands.ts          # typed wrappers over invoke()
│  └─ settings.ts
└─ components/
   ├─ SubtitleView.svelte  # the two-line bilingual display
   ├─ ControlBar.svelte    # start/stop, mode, status (hidden in click-through)
   └─ Settings.svelte
```
