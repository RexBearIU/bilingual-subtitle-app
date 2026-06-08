# Setup

## Post-install setup (end users)

If you installed from the **release `.exe`**, skip the dev-build prerequisites below
and follow these steps instead.

### 1 — Install Python 3.10+

Download from [python.org](https://www.python.org/downloads/) and tick
**"Add Python to PATH"** during install.

### 2 — Install faster-whisper and its dependencies

```powershell
pip install faster-whisper fastapi uvicorn ctranslate2
```

> On first launch, the Whisper medium model (~1.5 GB) downloads automatically from
> HuggingFace. This takes a few minutes. The ASR status dot will show **loading**
> until the download is complete.

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

### faster-whisper (ASR — Python sidecar)

The ASR backend is `faster_whisper_srv.py` — a Python HTTP server wrapping the
`faster-whisper` library.  On first run it downloads the model automatically
from HuggingFace (~1.5 GB for medium).

**Step 1 — Install Python 3.10+ and dependencies:**

```powershell
# Verify Python is available
python --version   # expect 3.10+

# Install required packages (once)
pip install faster-whisper fastapi uvicorn ctranslate2
```

**Step 2 — Set env vars** (user-level, persists across terminals):

| Env var | Default in code | Description |
|---------|-----------------|-------------|
| `PYTHON_BIN` | `python` | Python interpreter to use |
| `WHISPER_SERVER_SCRIPT` | `faster_whisper_srv.py` | Path to the server script |
| `WHISPER_MODEL` | `Systran/faster-whisper-medium` | HuggingFace repo ID **or** local path |
| `WHISPER_ASR_PORT` | `9001` | HTTP port |

```powershell
$proj = "C:\Users\User\.claude\projects\Bilingual Subtitle App"
[System.Environment]::SetEnvironmentVariable("WHISPER_SERVER_SCRIPT", "$proj\faster_whisper_srv.py", "User")
[System.Environment]::SetEnvironmentVariable("WHISPER_MODEL",         "Systran/faster-whisper-medium", "User")
[System.Environment]::SetEnvironmentVariable("WHISPER_ASR_PORT",      "9001",                          "User")
# PYTHON_BIN defaults to "python" — only set if you use a venv:
# [System.Environment]::SetEnvironmentVariable("PYTHON_BIN", "C:\path\to\venv\Scripts\python.exe", "User")
```

**Smoke-test:**

```powershell
$proj = "C:\Users\User\.claude\projects\Bilingual Subtitle App"
python "$proj\faster_whisper_srv.py" --model Systran/faster-whisper-medium --port 9001
# First run downloads ~1.5 GB — wait for "Uvicorn running on http://127.0.0.1:9001"
# In another terminal:
Invoke-WebRequest http://127.0.0.1:9001/   # should return 200
```

**Alternative models:**

| Model | Size | Notes |
|-------|------|-------|
| `Systran/faster-whisper-small` | ~500 MB | Faster, lower accuracy |
| `Systran/faster-whisper-medium` | ~1.5 GB | **Default** — good balance |
| `Systran/faster-whisper-large-v3` | ~3 GB | Best quality |

To use a local path instead of a HuggingFace ID, set `WHISPER_MODEL` to the
directory containing the model files.

**GPU acceleration:** faster-whisper uses CTranslate2. On Windows with an NVIDIA
GPU, `pip install ctranslate2` picks up CUDA automatically.  The `binaries/`
directory is added to the DLL search path by the script so the cublas DLLs that
ship with llama-server's Vulkan build are also found.

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
