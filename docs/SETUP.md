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

## Sidecar binaries & models (needed from Milestone 4 onward)

These are downloaded once and placed under `src-tauri/binaries/` and `models/`
(both git-ignored). See [docs/MILESTONES.md](MILESTONES.md) M4/M5 for exact files.

- `whisper-server.exe` + `ggml-medium.bin` (or `ggml-large-v3-turbo`)
- `llama-server.exe` + a Qwen GGUF (e.g. `Qwen2.5-1.5B-Instruct-Q4_K_M.gguf`)
