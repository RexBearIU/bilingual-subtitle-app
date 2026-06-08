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
