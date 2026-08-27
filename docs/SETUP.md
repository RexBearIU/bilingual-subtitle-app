# Setup

## Post-install setup (end users)

If you installed from the **release `.exe`**, skip the dev-build prerequisites below
and follow these steps instead.

### 1 — Install Python 3.10+

Download from [python.org](https://www.python.org/downloads/) and tick
**"Add Python to PATH"** during install.

### 2 — Install faster-whisper and its dependencies

```powershell
pip install faster-whisper fastapi uvicorn python-multipart ctranslate2
```

> On first launch, the Whisper large-v3-turbo model (~1.5 GB) downloads
> automatically from HuggingFace. This takes a few minutes. The ASR status dot
> will show **loading** until the download is complete.

### 3 — Set an OpenRouter API key

Translation calls a hosted model, so it needs a key from
<https://openrouter.ai/keys>. There is no model to download.

Easiest: launch the app, open **Settings ⚙️**, paste the key, press 儲存. It is
stored in `%APPDATA%\com.bilingualsubtitle.app\settings.json`.

Or put it in a gitignored `.env` at the repo root — copy the template and fill
in the blank:

```powershell
Copy-Item .env.example .env
notepad .env
```

Or set it as a real environment variable, which takes priority over both `.env`
and the stored key:

```powershell
[System.Environment]::SetEnvironmentVariable("OPENROUTER_API_KEY", "sk-or-v1-...", "User")
```

Resolution order is: real environment → `.env` → `settings.json`.

Without a key the app still runs — ASR works and subtitles show the source text
only, with the translation status dot red.

### 4 — Launch

Find **Bilingual Subtitles** in the Start menu (or the install directory) and run it.
The two status dots in the overlay should turn green within ~30 s on first run
(longer on very first launch while the Whisper model downloads).

---

## Prerequisites (Windows native — **not** WSL)

WASAPI loopback and the overlay window require a native Windows build. Do not
build or run inside WSL.

| Tool | Required | Notes |
|------|----------|-------|
| Windows 10/11 | ✅ | WASAPI loopback needs Win10 1803+ |
| WebView2 Runtime | ✅ | Ships with modern Windows; Tauri needs it |
| MSVC C++ Build Tools | ✅ | "Desktop development with C++" workload; provides the MSVC linker |
| Rust (stable, MSVC toolchain) | ✅ | `rustup default stable-msvc` |
| Node.js LTS + npm | ✅ | Frontend tooling (Vite) |
| Tauri CLI | ✅ | `cargo install tauri-cli --version "^2"` or `npm i -D @tauri-apps/cli` |
| CMake | ⛔ (not for sidecar path) | Only needed if/when we move ASR to native FFI |
| CUDA Toolkit | ⛔ | **Not needed** — whisper-cublas zip is self-contained |

### Verify environment

```powershell
rustc --version          # expect stable-*-msvc
cargo --version
node --version           # LTS
npm --version
cargo tauri --version    # Tauri v2
```

### This machine (recorded 2026-06-06)

- ✅ git, WebView2 (148.x), VS Build Tools 2026 (MSVC C++), winget
- Installed via winget during setup: Rust (rustup), Node.js LTS
- After install, open a fresh terminal so PATH updates take effect.

## First build (once scaffolded)

```powershell
npm install              # frontend deps
cargo tauri dev          # run the overlay in dev mode
```

## Sidecar binaries & models (M4 onward)

Models and binaries are git-ignored (`/binaries/`, `/models/`).

### ASR — Python sidecar (`asr_srv.py`)

The ASR backend is `asr_srv.py` — a Python HTTP server that supports two backends:

| Backend | Engine | Korean accuracy | GPU |
|---------|--------|-----------------|-----|
| `whisper` (default) | faster-whisper (CTranslate2) | moderate | yes, via CUDA |
| `sensevoice` | SenseVoice ONNX (sherpa-onnx) | excellent | CPU only (fast enough) |

**Step 1 — Install Python 3.10+ and dependencies:**

```powershell
python --version   # expect 3.10+

# whisper backend
pip install faster-whisper fastapi uvicorn python-multipart ctranslate2

# sensevoice backend (additional)
pip install sherpa-onnx
```

**Step 2 — Set env vars** (user-level, persists across terminals):

| Env var | Default | Description |
|---------|---------|-------------|
| `PYTHON_BIN` | `python` | Python interpreter |
| `ASR_BACKEND` | `whisper` | `whisper` or `sensevoice` |
| `ASR_SERVER_SCRIPT` | `asr_srv.py` | Path to the server script |
| `WHISPER_MODEL` | `deepdml/faster-whisper-large-v3-turbo-ct2` | HuggingFace repo ID (whisper backend) |
| `SENSEVOICE_MODEL` | `csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17` | HuggingFace repo ID (sensevoice backend) |
| `ASR_PORT` | `9001` | HTTP port |

```powershell
$proj = "C:\Users\User\.claude\projects\Bilingual Subtitle App"

# To use whisper backend (default):
[System.Environment]::SetEnvironmentVariable("WHISPER_MODEL", "deepdml/faster-whisper-large-v3-turbo-ct2", "User")

# To switch to SenseVoice (better Korean):
[System.Environment]::SetEnvironmentVariable("ASR_BACKEND", "sensevoice", "User")
```

**Smoke-test:**

```powershell
$proj = "C:\Users\User\.claude\projects\Bilingual Subtitle App"
python "$proj\asr_srv.py" --backend whisper --port 9001
# First run downloads the model — wait for "Ready on http://127.0.0.1:9001"
# In another terminal:
Invoke-WebRequest http://127.0.0.1:9001/   # should return 200
```

**Whisper model options:**

| Model | Download | VRAM | Notes |
|-------|----------|------|-------|
| `deepdml/faster-whisper-large-v3-turbo-ct2` | ~1.5 GB | ~1.6 GB fp16 | **Default ("turbo")** — public mirror (Systran turbo repo is now HF-gated) |
| `Systran/faster-whisper-large-v3` | ~3 GB | ~1.5 GB int8_float16 | **"large" in settings** — best quality, esp. Korean |

The settings panel cycles the **辨識引擎** button through **Whisper →
SenseVoice → Zipformer-KO**, and switches **turbo / large** (whisper) and
**int8 / fp32** (SenseVoice) without env vars — the idle asr-srv is killed and
relaunched with the new model on the next Start. A `WHISPER_MODEL` env var
overrides the whisper choice.

**Korean Zipformer backend (`zipformer-ko`):** a Korean-only sherpa-onnx
transducer (KsponSpeech). CPU real-time (~0.25 s for 25 s), full-length
transcription, natural conversational Korean; weaker than whisper large-v3 on
loanwords / code-switching. The model (~110 MB) auto-downloads on first Start to
`~/.cache/bilingual-subtitle/`; set `ZIPFORMER_MODEL` to a local model directory
to override. **Shares the sherpa-onnx runtime with SenseVoice**, so `PYTHON_BIN`
must point at a Python with `sherpa-onnx`, `fastapi`, `uvicorn`, and
`python-multipart` installed (the whisper backend instead needs `faster-whisper`).

**GPU acceleration:** faster-whisper uses CTranslate2 with CUDA automatically when
an NVIDIA GPU is present.  SenseVoice and Zipformer-KO run on CPU (ONNX) and are
already faster than real-time, so GPU is not needed for those backends.

### Translation via OpenRouter (M5)

No binaries, no model download, no GPU budget — translation is an HTTPS call.

Get a key at <https://openrouter.ai/keys>, then either paste it into
**Settings ⚙️** in the app, or set it in the environment:

```powershell
[System.Environment]::SetEnvironmentVariable("OPENROUTER_API_KEY", "sk-or-v1-...", "User")
```

Smoke-test the key and model without launching the app:

```powershell
$body = @{
  model    = "google/gemini-2.5-flash-lite"
  messages = @(@{ role = "user"; content = "Translate to Traditional Chinese: it's a bit overcast" })
} | ConvertTo-Json -Depth 5

Invoke-RestMethod -Method Post "https://openrouter.ai/api/v1/chat/completions" `
  -Headers @{ Authorization = "Bearer $env:OPENROUTER_API_KEY" } `
  -ContentType "application/json" -Body $body |
  ForEach-Object { $_.choices[0].message.content }
```

**Choosing a model.** Subtitles are one or two sentences and latency is what you
feel, so a small fast model beats a large one here. Anything on
<https://openrouter.ai/models> works — set it in Settings ⚙️ or via
`OPENROUTER_MODEL`. Note that the app sends the previous 3 subtitle pairs as
context on every call, so cost scales with roughly 4× the visible text.

**Offline / gaming scenario.** Translation needs network access. If the call
fails or no key is set, ASR keeps running and the overlay shows source-only
subtitles rather than stopping.

_M5 env vars (all optional — Settings ⚙️ covers the common ones):_

| Env var | Default in code | Description |
|---------|-----------------|-------------|
| `OPENROUTER_API_KEY` | — | API key; overrides the one in settings.json |
| `OPENROUTER_MODEL` | `google/gemini-2.5-flash-lite` | Model slug |
| `OPENROUTER_BASE_URL` | `https://openrouter.ai/api/v1` | Point at a proxy or an OpenAI-compatible gateway |
| `OPENROUTER_PROVIDER_ORDER` | — | Comma-separated upstream provider preference |
