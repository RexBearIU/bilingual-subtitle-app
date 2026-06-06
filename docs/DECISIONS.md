# Architecture Decision Records

Short, dated records of non-obvious choices and *why*. Append new ones; don't
rewrite history — supersede instead.

---

## ADR-0001 — Sidecar-first for native engines (whisper.cpp / llama.cpp)

**Date:** 2026-06-06 · **Status:** Accepted

**Context.** whisper.cpp and llama.cpp are C/C++. Two integration paths: (a) Rust
FFI / native linking, (b) run their bundled HTTP servers as Tauri sidecars.
Building from source on Windows needs CMake + a C++ toolchain and is the most
likely place to lose days.

**Decision.**
- **Translation (Qwen): `llama-server` sidecar, permanently.** It exposes an
  OpenAI-compatible HTTP API, keeps the model + KV cache resident, supports GPU
  builds, and translation payloads are tiny strings → HTTP cost is negligible.
  This is also the long-term answer, not a stepping stone.
- **ASR (whisper): `whisper-server` sidecar for v1**, to avoid the native build
  toolchain and ship fast using official prebuilt Windows binaries.

**Consequence / known trade-off.** ASR chunks are 16 kHz PCM arrays. Over HTTP
they must be serialized (WAV) per request, adding per-chunk overhead. If that
latency becomes the bottleneck, migrate **only ASR** to in-process
[`whisper-rs`](https://crates.io/crates/whisper-rs) FFI (audio buffers passed
directly, no serialization). Change one engine at a time; never both at once.

---

## ADR-0002 — WASAPI loopback, not cpal

**Date:** 2026-06-06 · **Status:** Accepted

**Context.** The spec suggested cpal for cross-platform audio. cpal's Windows
loopback support has historically been weak/unstable, and this app is Windows-only.

**Decision.** Capture the default render endpoint in loopback mode using the
[`wasapi`](https://crates.io/crates/wasapi) crate (or `windows-rs` directly with
`AUDCLNT_STREAMFLAGS_LOOPBACK`). No cpal.

**Consequence.** Capture code is Windows-specific by design. Acceptable — the
overlay and loopback are both inherently Windows-native (see ADR-0003).

---

## ADR-0003 — Windows-native only; no WSL

**Date:** 2026-06-06 · **Status:** Accepted

WASAPI loopback and the transparent always-on-top overlay both require a native
Windows host. All build/run happens on Windows. WSL is explicitly unsupported.

---

## ADR-0004 — Frontend: Svelte + Vite (no SvelteKit)

**Date:** 2026-06-06 · **Status:** Accepted

The app is a single transparent overlay with no routing, no SSR, no server. Plain
Svelte + Vite is lighter and sufficient. Revisit only if a multi-page settings
surface justifies routing.

---

## ADR-0005 — Dev injection instead of a "mock" stage

**Date:** 2026-06-06 · **Status:** Accepted

**Context.** The overlay (M1) must be testable before audio/ASR (M2/M4) exist, but
the user wants real implementation, not throwaway mock code.

**Decision.** No fake-subtitle product feature. Instead a dev-only command
`dev_inject_subtitle` emits a **real** `subtitle_update` through the **real** event
path — only the data source is manual during early milestones. When M4 lands, real
ASR output flows through the identical path; the dev command is feature-gated out
of release builds. Nothing gets thrown away.
