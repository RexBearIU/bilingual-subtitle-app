# Milestones

**A historical record, not current truth.** Every milestone below is done, and
the code has moved since several of them were written. For how anything behaves
today read [ARCHITECTURE.md](ARCHITECTURE.md); for why, the ADR index in
[DECISIONS.md](DECISIONS.md). This file is worth opening for one thing: what was
tried and measured at the time, which the other two do not carry.

It is deliberately **not** updated when behaviour changes later — a milestone
says what was true when it shipped. Superseding notes are added only where the
old text would otherwise be read as current.

Status legend: ⬜ not started · 🟡 in progress · ✅ done

| # | Milestone | Status |
|---|-----------|--------|
| 0 | Repo scaffold, docs, toolchain | ✅ |
| 1 | Tauri overlay shell | ✅ |
| 2 | WASAPI system audio capture | ✅ |
| 3 | Audio chunking + VAD | ✅ |
| 4 | Local ASR (whisper.cpp) | ✅ |
| 5 | Translation engine (Qwen) | ✅ |
| 6 | Subtitle state manager | ✅ |
| 7 | Product settings | ✅ |
| 8 | Performance optimization | ✅ |
| 9 | Optional SenseVoice backend | ✅ |

---

## M0 — Scaffold & toolchain  ✅

- [x] Decide stack & record ADRs ([DECISIONS.md](DECISIONS.md))
- [x] Write planning docs
- [x] Install Rust (rustup 1.29 / rustc 1.96 msvc) + Node LTS (24.16)
- [x] Scaffold Tauri v2 + Svelte + Vite (TS) project
- [x] `cargo check` clean · `npm run check` clean
- [x] `git init` on `main`, `.gitignore` (models/ + binaries/ excluded)

## M1 — Tauri overlay shell  ✅

**Build the real event pipeline end-to-end with a dev injection source (ADR-0005).**

Frontend (`src/`):
- [x] Transparent, frameless, always-on-top window (`tauri.conf.json`)
- [x] Click-through toggle (`set_ignore_cursor_events`)
- [x] Draggable subtitle area (`data-tauri-drag-region`)
- [x] Font-size setting (slider → `set_font_size`)
- [x] Subtitle-mode setting (zh-ko / zh-en segmented control)
- [x] Start / stop button
- [x] Model/engine status display (status dots)
- [x] **Visual verification via `npm run tauri dev`** — transparent ✓, draggable ✓,
      mode switch ✓, inject ✓, on-top toggle ✓, click-through + recovery ✓
- [x] System tray (checkable 穿透/置頂 + 結束) and `Ctrl+Alt+P` escape hatch
- [x] Click-through hides control bar (clean caption-only overlay)

Backend commands (`src-tauri/src/commands.rs`): `start_captioning`,
`stop_captioning`, `set_subtitle_mode`, `set_click_through`, `set_font_size`,
`get_status`, `dev_inject_subtitle`. Event: `subtitle_update` + `engine_status`
(see [IPC-CONTRACT.md](IPC-CONTRACT.md)).

> **Click-through lockout — solved two ways:** enabling click-through makes the
> whole window pass-through, so no in-overlay button is clickable. Recovery is
> therefore handled *outside* the overlay:
> 1. **System tray icon** (`TrayIconBuilder` in `lib.rs`) — always clickable.
>    Menu: 停用穿透 / 切換置頂 / 結束. This is the primary "escape button".
> 2. **Global hotkey `Ctrl+Alt+P`** (`tauri-plugin-global-shortcut`) — backup.
>
> Both call the shared `force_interactive()` helper. Also: `set_always_on_top`
> command + 📌 toggle button (pin/unpin) in the control bar.

**Acceptance:** opens as transparent overlay · displays injected (real-path)
subtitles · zh-ko/zh-en switch works · stays above browser/video.

## M2 — WASAPI capture  ✅

Modules: `audio/{mod,capture,resample,meter}.rs`.
**Acceptance:** YouTube playback → non-zero captured audio · RMS shown in
debug/UI · no mic · no WSL.

**Verified:** WASAPI loopback stream at 192 kHz / 2 ch / 32 bps f32.
Start→stop lifecycle clean. RMS emitted to frontend via `engine_status`.
Tauri hot-rebuild round-trip 6.6 s.

## M3 — Audio chunking  ✅

Fixed-chunk accumulator, replacing the original RMS VAD (ADR-0009).
16 kHz mono · 4 s chunks · stop-flush ≥ 0.5 s.

**Acceptance:** audio reaches ASR in regularly-sized chunks · silence handled
by Whisper `no_speech_prob` · no memory growth over long sessions.

## M4 — ASR  ✅

Sidecar ASR, loaded once, returning text plus a detected language, with prior
context carried across chunks. Began as a `whisper-server` binary (ADR-0001)
and became the Python `asr_srv.py` during M8 (ADR-0006).

**Acceptance:** ko/en/zh transcribed · language auto-detected · source subtitle
emitted without waiting for translation.

**Worth keeping:** `ureq` v2 was chosen for the HTTP client specifically
because it is synchronous — an async client would have dragged tokio into a
crate that has no other use for it. Server readiness is polled for 300 s, which
looks absurd until the first run downloads a 1.5 GB model.

## M5 — Translation  ✅

Subtitle-style translation: output the line and nothing else, natural register,
keep names and common English technical terms, no explanations. Shipped against
a local `llama-server` sidecar running Qwen3-4B.

**Acceptance:** ko→zh · en→zh · zh→ko/en · latency low enough to read.

> **Superseded by [ADR-0011](DECISIONS.md) (2026-08-27).** The local sidecar is
> gone; translation is an HTTPS call. The channel boundary, the prompt design,
> the rolling context and the source-first-then-translation emit order all
> survived the move unchanged.

**Worth keeping:** Qwen3 needed an explicit `/no_think` directive or it spent
its token budget reasoning and returned an empty translation.
`strip_think_tags` remains as a safety net for any model that emits a `<think>`
block regardless.

## M6 — Subtitle state manager  ✅

Dedup by id, merge partial into final, expire after a few seconds, cap how many
show at once, keep the last N as translation context.

**Acceptance:** no flicker · no duplicate text · subtitles disappear naturally.

## M7 — Settings  ✅

Persisted to `{AppData}/com.bilingualsubtitle.app/settings.json` with
`serde_json` and `std::fs` — no store plugin.

**Acceptance:** settings survive restart · mode changeable while running.

**Worth keeping:** window geometry is saved from the frontend on `onMoved` /
`onResized` with a 400 ms debounce, because the events fire continuously
during a drag.

## M8 — Performance  ✅

Targets: 1–3 s end-to-end · low idle CPU · models stay loaded · no memory
growth. Separate worker threads, bounded channels, stale chunks dropped under
back-pressure.

Most of what landed here has been rewritten since; ARCHITECTURE is current for
all of it. What is recorded below is the part that was *discovered* rather than
designed, and would cost the same debugging to learn twice.

- **`IsSystemSoundsSession()` returns `Ok(())` for both S_OK and S_FALSE** in
  windows-rs, so treating it as a boolean filtered out every audio session and
  the process picker came up empty. Fixed by checking PID 0 instead. Nothing
  about the call site suggests this; it looks correct.
- **Bounded channels, and where not to bound them.** VAD→ASR and
  ASR→Translation are `sync_channel` with `try_send`, so a busy consumer drops
  a chunk with a WARN rather than growing a queue. Capture→VAD stays unbounded
  on purpose — it is pure RMS arithmetic and never the bottleneck. The
  chunker→ASR capacity went 2 → 4 after normal GPU inference latency proved
  enough to drop chunks at 2.
- **The RMS meter logged at INFO every 200 ms** — five lines a second, which
  buried everything else. At DEBUG now, along with `level_for()` suppressors on
  `ureq`, `wasapi`, `tauri`, `tao` and `wry`; without those, a debug build's log
  is unreadable exactly when it is needed.
- **Sidecars outlive a crash.** `kill_port()` runs `netstat -ano` + `taskkill`
  before each launch, because a force-killed session leaves a process holding
  the port and the next start fails in a way that looks like a code bug.
- **`condition_on_previous_text=True`** stayed on, but not for the reason first
  recorded here. A live A/B appeared to show that turning it off doubled
  final-chunk latency (376 ms → 739 ms median); a controlled sweep over the
  same audio (`bench/sweep.py`) found no latency difference at all — 391 ms
  against baseline's 406 ms — and identical text. The live comparison had been
  reading content differences as a parameter effect.

  What the sweep did find: on music it raises run-to-run instability sharply
  (0.106 → 0.322), so it is doing something, just not within a single short
  chunk. It conditions on previous *segments inside one `transcribe()` call*,
  and the app sends one utterance per call.

## M9 — SenseVoice backend  ✅

SenseVoice via `sherpa-onnx` as an alternative ASR backend (ADR-0010), selected
by `ASR_BACKEND=sensevoice`, sharing the whole downstream pipeline.

**Worth keeping:** ~70× faster than real-time on CPU, so no GPU budget at all.
Its `result.event` field reports BGM / Applause / Laughter and is used for
noise gating — the only music detector already present in this codebase.
Language normalization has to handle full English names (`"Korean"` → `"ko"`),
which the whisper backend never emits.
