# Local Realtime Bilingual Subtitle App

A Windows desktop overlay that captures **system output audio** (WASAPI loopback),
transcribes speech **locally** (whisper.cpp), translates it **locally** (Qwen via
llama.cpp), and renders **bilingual subtitles** in a transparent, always-on-top,
click-through window.

- **100% local.** No cloud API, no account, no network dependency at runtime
  (model/binary downloads happen once at setup).
- **Any source app.** Browser video/live streams, Discord, VLC, games — anything
  that plays through the Windows default output device.
- **Subtitle modes:** translate to `zh` / `ko` / `en`, or `none` (source text only).
- **Per-process capture:** target a single app (e.g. a game) instead of all system audio.
- **Music mode:** bypasses VAD for continuous lyrics captioning.

> Status: **M0–M8 complete** (overlay · WASAPI capture · VAD · ASR · translation · subtitle store · settings · performance). M9 (SenseVoice) optional — see [docs/MILESTONES.md](docs/MILESTONES.md).

## Download & install

1. Download **`Bilingual Subtitles_0.1.0_x64-setup.exe`** from the [latest release](https://github.com/RexBearIU/bilingual-subtitle-app/releases/latest).
2. Run the installer (current-user install, no admin required).
3. Follow the **[post-install setup in SETUP.md](docs/SETUP.md#post-install-setup-end-users)** to install Python dependencies and download models (~4 GB total).

## Tech stack

| Layer | Choice |
|-------|--------|
| Shell | Tauri v2 |
| Backend | Rust |
| Frontend | Svelte 5 + Vite (no SvelteKit — single overlay, no routing needed) |
| Audio capture | Windows WASAPI loopback + per-process loopback (`process_loopback.rs`) |
| ASR | faster-whisper — Python HTTP sidecar (`faster_whisper_srv.py`) |
| Translation | Qwen3-4B GGUF via `llama-server` sidecar (OpenAI-compatible HTTP, Vulkan GPU) |
| Later | SenseVoice ASR backend (optional, M9) |

## Documentation

- [docs/SETUP.md](docs/SETUP.md) — prerequisites & first build
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — pipeline, threads, data flow
- [docs/MILESTONES.md](docs/MILESTONES.md) — roadmap with acceptance criteria & status
- [docs/DECISIONS.md](docs/DECISIONS.md) — architecture decision records (ADRs)
- [docs/IPC-CONTRACT.md](docs/IPC-CONTRACT.md) — Tauri commands & events (frontend↔backend API)

## Out of scope (MVP)

Chrome extension · mobile · cloud API · accounts · payment · OBS plugin ·
speech-to-speech · recording · subtitle export.
