# Local Realtime Bilingual Subtitle App

A Windows desktop overlay that captures **system output audio** (WASAPI loopback),
transcribes speech **locally** (whisper.cpp), translates it **locally** (Qwen via
llama.cpp), and renders **bilingual subtitles** in a transparent, always-on-top,
click-through window.

- **100% local.** No cloud API, no account, no network dependency at runtime
  (model/binary downloads happen once at setup).
- **Any source app.** Browser video/live streams, Discord, VLC, games — anything
  that plays through the Windows default output device.
- **Subtitle modes:** `zh-ko` and `zh-en`.

> Status: **Milestone 1 (Tauri overlay shell)** — see [docs/MILESTONES.md](docs/MILESTONES.md).

## Tech stack

| Layer | Choice |
|-------|--------|
| Shell | Tauri v2 |
| Backend | Rust |
| Frontend | Svelte + Vite (no SvelteKit — single overlay, no routing needed) |
| Audio capture | Windows WASAPI loopback via `wasapi` crate (**not** cpal) |
| ASR | whisper.cpp — `whisper-server` sidecar first, `whisper-rs` FFI later |
| Translation | Qwen GGUF via `llama-server` sidecar (OpenAI-compatible HTTP) |
| Later | SenseVoice ASR backend (optional) |

## Documentation

- [docs/SETUP.md](docs/SETUP.md) — prerequisites & first build
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — pipeline, threads, data flow
- [docs/MILESTONES.md](docs/MILESTONES.md) — roadmap with acceptance criteria & status
- [docs/DECISIONS.md](docs/DECISIONS.md) — architecture decision records (ADRs)
- [docs/IPC-CONTRACT.md](docs/IPC-CONTRACT.md) — Tauri commands & events (frontend↔backend API)

## Out of scope (MVP)

Chrome extension · mobile · cloud API · accounts · payment · OBS plugin ·
speech-to-speech · recording · subtitle export.
