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

### 3 — Download the Qwen3-4B translation model

```powershell
# ~2.4 GB — run once
$dest = "$env:APPDATA\BilingSubs\models"
New-Item -ItemType Directory -Force $dest | Out-Null
Invoke-WebRequest `
  "https://huggingface.co/bartowski/Qwen3-4B-GGUF/resolve/main/Qwen3-4B-Q4_K_M.gguf" `
  -OutFile "$dest\Qwen3-4B-Q4_K_M.gguf" -UseBasicParsing
```

Then tell the app where the model is (run once in a terminal):

```powershell
[System.Environment]::SetEnvironmentVariable(
  "LLAMA_MODEL",
  "$env:APPDATA\BilingSubs\models\Qwen3-4B-Q4_K_M.gguf",
  "User"
)
```

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

### llama-server (translation, M5)

**No CUDA Toolkit required** — use the **Vulkan** build (self-contained, ships
with any NVIDIA driver).

```powershell
# Download Vulkan build (check https://github.com/ggml-org/llama.cpp/releases for latest)
$ProgressPreference = 'SilentlyContinue'
Invoke-WebRequest `
  "https://github.com/ggml-org/llama.cpp/releases/download/b9542/llama-b9542-bin-win-vulkan-x64.zip" `
  -OutFile "$env:TEMP\llama-vulkan.zip" -UseBasicParsing
Expand-Archive "$env:TEMP\llama-vulkan.zip" -DestinationPath "$env:TEMP\llama-vulkan" -Force

$proj = "C:\Users\User\.claude\projects\Bilingual Subtitle App"
Get-ChildItem "$env:TEMP\llama-vulkan" -Recurse |
    Where-Object { $_.Extension -in ".exe",".dll" } |
    ForEach-Object { Copy-Item $_.FullName "$proj\binaries\$($_.Name)" -Force }
```

Download Qwen3-4B model (~2.4 GB):

```powershell
$proj = "C:\Users\User\.claude\projects\Bilingual Subtitle App"
Invoke-WebRequest `
  "https://huggingface.co/bartowski/Qwen3-4B-GGUF/resolve/main/Qwen3-4B-Q4_K_M.gguf" `
  -OutFile "$proj\models\Qwen3-4B-Q4_K_M.gguf" -UseBasicParsing
```

Set env vars:

```powershell
$proj = "C:\Users\User\.claude\projects\Bilingual Subtitle App"
[System.Environment]::SetEnvironmentVariable("LLAMA_SERVER_BIN",  "$proj\binaries\llama-server.exe",    "User")
[System.Environment]::SetEnvironmentVariable("LLAMA_MODEL",       "$proj\models\Qwen3-4B-Q4_K_M.gguf", "User")
[System.Environment]::SetEnvironmentVariable("LLAMA_PORT",        "9002",                               "User")
[System.Environment]::SetEnvironmentVariable("LLAMA_GPU_LAYERS",  "36",                                 "User")
```

Smoke-test:

```powershell
$proj = "C:\Users\User\.claude\projects\Bilingual Subtitle App"
& "$proj\binaries\llama-server.exe" `
  -m "$proj\models\Qwen3-4B-Q4_K_M.gguf" `
  --port 9002 -ngl 36 -c 2048 --no-webui
# In another terminal:
# Invoke-WebRequest http://127.0.0.1:9002/health   → {"status":"ok"}
# Then POST /v1/chat/completions with /no_think prompt
```

**Gaming scenario** (free VRAM for the game): set `LLAMA_GPU_LAYERS=0` to run
translation on CPU only. Latency increases ~3× but typically stays under 1s for
subtitle-length text.

_M5 env vars:_

| Env var | Default in code | Description |
|---------|-----------------|-------------|
| `LLAMA_SERVER_BIN` | `llama-server` (PATH) | Path to llama-server.exe |
| `LLAMA_MODEL` | `models/Qwen3-4B-Q4_K_M.gguf` | Path to Qwen3 GGUF model |
| `LLAMA_PORT` | `9002` | HTTP port for llama-server |
| `LLAMA_GPU_LAYERS` | `36` | GPU offload layers (0=CPU, 36=all GPU) |
