# Setup

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

All files are git-ignored (`/binaries/`, `/models/`).  
**Three env vars** must be set (user-level, persists across terminals):

| Env var | Default in code | Machine value |
|---------|-----------------|---------------|
| `WHISPER_SERVER_BIN` | `whisper-server` (PATH) | `<project>\binaries\whisper-server.exe` |
| `WHISPER_MODEL` | `models/ggml-medium.bin` | `<project>\models\ggml-medium.bin` |
| `WHISPER_ASR_PORT` | `9001` | `9001` |

Set them once:

```powershell
$proj = "C:\Users\User\.claude\projects\Bilingual Subtitle App"
[System.Environment]::SetEnvironmentVariable("WHISPER_SERVER_BIN", "$proj\binaries\whisper-server.exe", "User")
[System.Environment]::SetEnvironmentVariable("WHISPER_MODEL",      "$proj\models\ggml-medium.bin",      "User")
[System.Environment]::SetEnvironmentVariable("WHISPER_ASR_PORT",   "9001",                               "User")
```

### whisper-server (ASR)

**No CUDA Toolkit installation required** — the cublas zip bundles its own
CUDA runtime DLLs (`cublas64_12.dll`, `cublasLt64_12.dll`, `cudart64_12.dll`, etc.).

```powershell
# Download GPU build (whisper.cpp v1.8.6, CUDA 12.4, self-contained)
$ProgressPreference = 'SilentlyContinue'
Invoke-WebRequest `
  "https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.6/whisper-cublas-12.4.0-bin-x64.zip" `
  -OutFile "$env:TEMP\whisper-cublas.zip" -UseBasicParsing
Expand-Archive "$env:TEMP\whisper-cublas.zip" -DestinationPath "$env:TEMP\whisper-cublas" -Force

$proj = "C:\Users\User\.claude\projects\Bilingual Subtitle App"
New-Item -ItemType Directory -Force "$proj\binaries" | Out-Null
Get-ChildItem "$env:TEMP\whisper-cublas\Release" | Where-Object { $_.Extension -in ".exe",".dll" } |
    ForEach-Object { Copy-Item $_.FullName "$proj\binaries\$($_.Name)" -Force }
```

Download medium model (~1.5 GB):

```powershell
$proj = "C:\Users\User\.claude\projects\Bilingual Subtitle App"
New-Item -ItemType Directory -Force "$proj\models" | Out-Null
$ProgressPreference = 'SilentlyContinue'
Invoke-WebRequest `
  "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin" `
  -OutFile "$proj\models\ggml-medium.bin" -UseBasicParsing
```

Smoke-test (should respond within ~15s for medium):

```powershell
$proj = "C:\Users\User\.claude\projects\Bilingual Subtitle App"
& "$proj\binaries\whisper-server.exe" -m "$proj\models\ggml-medium.bin" --port 9001 --language auto
# Open another terminal: Invoke-WebRequest http://127.0.0.1:9001/ — should return 200
```

### CPU fallback (no GPU / testing)

If you have no NVIDIA GPU or want to test without CUDA:

```powershell
Invoke-WebRequest `
  "https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.6/whisper-blas-bin-x64.zip" `
  -OutFile "$env:TEMP\whisper-blas.zip" -UseBasicParsing
# ... same extraction steps ...
# Use ggml-small.bin (~465 MB) for acceptable CPU latency
Invoke-WebRequest `
  "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin" `
  -OutFile "$proj\models\ggml-small.bin" -UseBasicParsing
[System.Environment]::SetEnvironmentVariable("WHISPER_MODEL", "$proj\models\ggml-small.bin", "User")
```

### llama-server (translation, M5)

```powershell
# Download llama.cpp release (check https://github.com/ggml-org/llama.cpp/releases for latest)
# Look for: llama-<version>-bin-win-cuda-cu12.4-x64.zip  (GPU)
#       or: llama-<version>-bin-win-openblas-x64.zip      (CPU)
# Extract llama-server.exe + DLLs to binaries/
# Download model from HuggingFace (Qwen2.5-1.5B-Instruct-Q4_K_M.gguf, ~1 GB)
```

_M5 env vars (to be set when implementing translation):_

| Env var | Default | Description |
|---------|---------|-------------|
| `LLAMA_SERVER_BIN` | `llama-server` | Path to llama-server.exe |
| `LLAMA_MODEL` | `models/qwen2.5-1.5b-q4.gguf` | Path to Qwen GGUF model |
| `LLAMA_PORT` | `9002` | HTTP port for llama-server |
