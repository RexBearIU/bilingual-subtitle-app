# Architecture

## Pipeline

```text
Windows default output device (or specific process via Process Loopback API)
        │  WASAPI loopback / Process Loopback (shared mode)
        ▼
[capture thread]  AudioPump: bytes → f32 → every 200 ms RMS + resample
        ▼
[resample]  → 16 kHz mono f32
        ▼
[chunker worker]  graduated silence flush / 6 s cap / rolling partials
        ▼
[ASR worker]  faster-whisper (Python HTTP sidecar); coalesces stale partials
        │  → { text, lang(ko|en|zh|ja), no_speech_prob }
        │  filters: no_speech_prob ≥ 0.7, blocklist, repeat-loop, script-based lang fix
        ▼
[Translation worker]  OpenRouter chat-completions (HTTPS); newest-first under backlog
        │  → target-language subtitle text (single language per mode)
        ▼
[Subtitle state manager]  dedup by id / partial→final merge / expiry pruning
        │
        ▼
Tauri event  `subtitle_update`
        ▼
Svelte transparent overlay (render-only)
```

## Thread / channel model

Each stage is its own worker, connected by **bounded channels**. Back-pressure
policy — when a stage falls behind, sacrifice the right thing:

```text
capture(thread) ──► chunker(worker) ──►[sync_channel(8)]──► asr(worker)
   ──►[sync_channel(4)]──► translate(worker) ──► state mgr ──► emit
```

- The ASR model is **loaded once** and kept resident for the whole session; the
  sidecar stays alive across Stop/Start cycles — no model reload. Translation
  holds no local state, just a `ureq` agent with a 12 s read timeout.
- Capture→chunker is an unbounded `mpsc::channel` (chunker is fast — pure accumulation).
- Chunker→ASR: **partials** use `try_send` and are dropped when full (disposable
  previews); **finals** use blocking `send` — a lost final is a lost subtitle.
- ASR worker **coalesces its backlog**: any partial with a newer chunk queued
  behind it is skipped without inference.
- Translation worker under backlog **skips to the newest request** — the visible
  line going untranslated is worse than an old line keeping its source text.
- No unbounded queues anywhere → no memory growth over long sessions.

## Process topology

```text
Tauri app (Rust)
 ├─ owns: WASAPI/process capture, chunker, state manager, UI events
 ├─ spawns: python asr_srv.py             (HTTP :9001)  ── ASR (whisper or sensevoice backend)
 └─ calls:  openrouter.ai/api/v1          (HTTPS)       ── translation (no local process)
```

Sidecars are launched on the first `start_captioning` call and stay alive until
the app exits (Drop impl sends SIGKILL). `kill_port()` in `commands.rs` evicts any
zombie sidecar from a previous session before each launch. See [DECISIONS.md](DECISIONS.md)
ADR-0001, ADR-0006 for sidecar rationale and the faster-whisper choice.

## Subtitle mode logic

`mode` selects the **target translation language**. The source language detected by
ASR is shown in its original slot; the target language is translated by Qwen.

```text
mode = "zh"   → translate source (ko/en/ja/…) to Traditional Chinese
mode = "ko"   → translate source to Korean
mode = "en"   → translate source to English
mode = "none" → source text only, no translation call
```

If the detected source language already matches the target, the translation worker
emits the source text as-is (no LLM call needed).

## Chunking (graduated silence flush + rolling partials)

The chunker (`pipeline/chunker.rs`) accumulates resampled 16 kHz mono samples.

**Video / stream mode (default):**
1. **Rolling partial flush** — after 1 s a copy of the buffer is sent
   (`is_partial=true`, beam_size=1), then an updated copy every further 1.5 s
   while the utterance continues. On-screen text keeps refreshing during long
   utterances; the buffer is never drained by a partial.
2. **Final flush** — triggered by the **graduated silence rule** or the 6 s cap.
   The more audio buffered, the shorter the pause needed to cut:

   | buffered audio | silence required |
   |---|---|
   | < 1.5 s | 800 ms (no micro-fragments from a breath) |
   | 1.5 – 2.5 s | 400 ms |
   | ≥ 2.5 s | 200 ms (cut at the first real dip) |

   The 6 s cap is only reached by pause-less speech (fast talkers), where the
   longer Whisper context helps most. A cap cut lands on the **quietest 50 ms
   window** in the last 1.5 s (not mid-word); the remainder seeds the next
   utterance.

**Music mode:** fixed 10 s chunks, no partial flush, beam_size=3 + "Song lyrics:" prompt.

Pure-silence buffers are discarded without an ASR call. A stop-flush sends any
accumulator ≥ 0.5 s when `stop_captioning` is called. The `speech_threshold`
setting is retained in IPC types for API compatibility but is no longer read.

## ASR worker

Default model `deepdml/faster-whisper-large-v3-turbo-ct2` (public ct2 mirror; the
original `Systran/faster-whisper-large-v3-turbo` repo is now HF-gated); the settings UI can switch
to `large-v3` (quantised `int8_float16`, ~1.5 GB VRAM). Env `WHISPER_MODEL`
overrides both. Runs on CUDA float16 when a GPU is detected; `without_timestamps`
is enabled (timestamps unused downstream, fewer hallucinations).

**beam_size strategy:**
- Partial chunks → beam_size=1 (greedy, fast preview)
- Final chunks (video) → beam_size=5 (accurate)
- Music mode → beam_size=3

**Script-based language correction:** Whisper's per-chunk language claim is
unreliable on short audio (Korean text labeled "en"). The dominant script of the
*output text* (Hangul/Han/Kana/Latin) overrides the claimed language when they
disagree, so translation prompts always carry the right source language.

## Translation worker

Calls OpenRouter's `/v1/chat/completions` (ADR-0011). The last **3
(source → translation) pairs** are replayed as chat turns for cross-subtitle
continuity (names, loanwords, omitted Korean subjects). The system prompt covers
ASR-error tolerance (no fragment completion), multi-speaker dash separation, and
a lyric register in music mode. Korean source adds loanword/name/register rules.
Requests set `reasoning.enabled = false`; `strip_think_tags` is kept as a
safety net for models that emit a `<think>` block anyway.

Failure handling, in order of cheapness:
- Punctuation-only input, or source language == target → no request at all.
- 429 / 5xx / transport error → **one** 250 ms retry, then give up for that line.
- Give-up and missing-key both leave the source-only subtitle on screen; they
  never block the pipeline.

Config resolves env-first (`OPENROUTER_API_KEY` / `_MODEL` / `_BASE_URL` /
`_PROVIDER_ORDER`), then `settings.json`. The key is held only in `RemoteConfig`,
never in `AppState` — `AppState` derives `Debug` and is logged on every change.

## Per-process capture

When a `capture_target` is set (via `set_capture_process`), the Rust backend uses
the Windows **Process Loopback API** (`ActivateAudioInterfaceAsync` +
`AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`) to capture audio from only that
process tree. Falls back to system-wide WASAPI loopback when no target is set.
Requires Windows 10 Build 20348 or Windows 11.

Chromium browsers recycle the audio-renderer subprocess that owns the WASAPI
session, so the picked PID can go stale; activating a dead PID fails with
`E_NOTIMPL`. Defences (in order): session enumeration lists **active** sessions
only; the PID is **re-resolved at Start**; one recovery retry re-enumerates; then
system loopback with the error stored in `AppState.loopback_error` for the UI.

## Hallucination filtering

The ASR worker applies three layers before emitting any subtitle:

1. **`no_speech_prob` filter** — faster-whisper returns a per-segment probability
   that the audio contains no speech. Segments with mean ≥ 0.7 are dropped.
2. **Blocklist filter** — known hallucination phrases (YouTube credits, `[Music]`,
   `[BLANK_AUDIO]`, etc.) are blocked by substring match.
3. **Repeat-loop filter** — *one* consecutive exact repeat is allowed (real
   echoed replies like "네." / "네." between speakers); the second consecutive
   repeat (< 60 chars) marks an `initial_prompt` feedback loop and is suppressed.
   Finals completing a partial of the same utterance are exempt.

## Backend module layout

```text
src-tauri/src/
├─ main.rs                    # Tauri builder entry point
├─ lib.rs                     # setup: managed state, tray, shortcuts, log filters
├─ commands.rs                # #[tauri::command] handlers; sidecar launch + kill_port()
├─ state.rs                   # AppState + update_and_emit/read_state helpers + AsrProc/LlamaProc
├─ types.rs                   # Shared IPC types (SubtitleMode, EngineStatus, …)
├─ settings.rs                # PersistSettings + OverlayRect; JSON file I/O
├─ util.rs                    # wait_for_http_ok (sidecar readiness polling)
├─ pipeline/
│  ├─ mod.rs                  # pub mod chunker
│  └─ chunker.rs              # Graduated silence flush / 6 s cap / rolling partials
├─ audio/
│  ├─ mod.rs
│  ├─ capture.rs              # System loopback + AudioPump (shared capture plumbing)
│  ├─ process_loopback.rs     # Per-process loopback (Windows Process Loopback API)
│  ├─ session_enum.rs         # List ACTIVE audio sessions (for process picker)
│  ├─ resample.rs             # → 16 kHz mono (rubato SincFixedIn)
│  └─ meter.rs                # RMS helper (used for UI level meter)
├─ asr/
│  ├─ mod.rs                  # AudioChunk type
│  └─ http_client.rs          # ASR HTTP client; filters; backlog coalescing
└─ translate/
   ├─ mod.rs                  # TranslationRequest + RemoteConfig resolution
   └─ openrouter.rs           # OpenRouter chat-completions client + retry
```

## Frontend layout

```text
src/
├─ App.svelte                 # overlay root (transparent, render-only)
├─ main.ts
├─ lib/
│  ├─ subtitles.svelte.ts     # OverlayStore: subscribe to events, hold render state
│  ├─ commands.ts             # typed wrappers over invoke()
│  └─ types.ts                # IPC types (mirrors src-tauri/src/types.rs)
└─ components/
   ├─ SubtitleView.svelte     # stacked bilingual subtitle display
   ├─ ControlBar.svelte       # start/stop, mode, status, settings trigger
   ├─ ProcessPicker.svelte    # per-process audio capture selector
   └─ SettingsPanel.svelte    # settings overlay (opacity, GPU layers)
```
