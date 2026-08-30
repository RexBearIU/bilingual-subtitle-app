# Local Realtime Bilingual Subtitle App

A Windows desktop overlay that captures **system output audio** (WASAPI loopback),
transcribes speech **locally** (faster-whisper, SenseVoice, or a Korean
Zipformer), translates it via **OpenRouter**, and renders
**bilingual subtitles** in a transparent, always-on-top, click-through window.

- **Local ASR.** Speech recognition runs entirely on your machine; audio never
  leaves it. Only the recognised *text* is sent out for translation.
- **No local LLM.** Translation calls a hosted model through OpenRouter, so
  there is no 2.5 GB GGUF to download and nothing competing for GPU VRAM with
  whatever you are watching or playing. Needs an API key and network access.
- **Any source app.** Browser video/live streams, Discord, VLC, games — anything
  that plays through the Windows default output device.
- **Subtitle modes:** translate to `zh` / `ko` / `en`, or `none` (source text only).
- **Per-process capture:** target a single app (e.g. a game) instead of all system audio.
- **Copy a line:** every subtitle carries a copy button; the caption stays click-through.

> Status: **feature-complete MVP** — overlay · system & per-process capture ·
> graduated chunking · ASR (whisper/SenseVoice/Zipformer-KO) · translation · settings ·
> backpressure-aware real-time pipeline.

## Download & install

1. Download **`Bilingual Subtitles_0.1.0_x64-setup.exe`** from the [latest release](https://github.com/RexBearIU/bilingual-subtitle-app/releases/latest).
2. Run the installer (current-user install, no admin required).
3. Follow the **[post-install setup in SETUP.md](docs/SETUP.md#post-install-setup-end-users)** to install Python dependencies and download the ASR model (~1.5 GB).
4. Put an [OpenRouter API key](https://openrouter.ai/keys) into Settings ⚙️ (or set
   `OPENROUTER_API_KEY`) — without one, subtitles show the source text only.

## Tech stack

| Layer | Choice |
|-------|--------|
| Shell | Tauri v2 |
| Backend | Rust |
| Frontend | Svelte 5 + Vite (no SvelteKit — single overlay, no routing needed) |
| Audio capture | Windows WASAPI loopback + per-process loopback (`process_loopback.rs`) |
| ASR | `asr_srv.py` Python sidecar — faster-whisper (default) or SenseVoice via sherpa-onnx (`ASR_BACKEND=sensevoice`) |
| Translation | OpenRouter `/v1/chat/completions` (see [ADR-0011](docs/DECISIONS.md)) |

## Documentation

- [docs/SETUP.md](docs/SETUP.md) — prerequisites & first build
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — pipeline, threads, data flow
- [docs/MILESTONES.md](docs/MILESTONES.md) — roadmap with acceptance criteria & status
- [docs/DECISIONS.md](docs/DECISIONS.md) — architecture decision records (ADRs)
- [docs/IPC-CONTRACT.md](docs/IPC-CONTRACT.md) — Tauri commands & events (frontend↔backend API)

## Out of scope (MVP)

Chrome extension · mobile · accounts · payment · OBS plugin ·
speech-to-speech · recording · subtitle export.

Cloud was out of scope until [ADR-0011](docs/DECISIONS.md) moved translation to
OpenRouter. ASR stays local — no speech-to-text API is involved, and captured
audio never leaves the machine.

## License

[MIT](LICENSE) © RexBearIU
