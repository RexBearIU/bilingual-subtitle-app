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
- Final chunks → beam_size=5 (accurate)

**Script-based language correction:** Whisper's per-chunk language claim is
unreliable on short audio (Korean text labeled "en"). The dominant script of the
*output text* (Hangul/Han/Kana/Latin) overrides the claimed language when they
disagree, so translation prompts always carry the right source language.

## Translation worker

Calls OpenRouter's `/v1/chat/completions` (ADR-0011). The last **3
(source → translation) pairs** are replayed as chat turns for cross-subtitle
continuity (names, loanwords, omitted Korean subjects). The system prompt covers
ASR-error tolerance (no fragment completion) and multi-speaker dash separation.
Korean source adds loanword/name/register rules.
Requests set `reasoning.enabled = false`; `strip_think_tags` is kept as a
safety net for models that emit a `<think>` block anyway.

**Translation cache** (64 lines, LRU, keyed on source_lang + mode + source
text): a preview and its final carry identical text whenever speech stops
before the wording changes, and repeated lines (choruses, catchphrases,
surviving hallucinations) recur on their own. Re-asking costs a round-trip and
can come back worded differently, which rewrites a subtitle already on screen.
The key omits the rolling context on purpose — on screen, stability beats
context-tuning. A hit still enters the history, so context stays complete.

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

The ASR worker applies four layers before emitting any subtitle:

1. **`no_speech_prob` filter** — faster-whisper returns a per-segment probability
   that the audio contains no speech. Segments with mean ≥ 0.7 are dropped.
   Weaker than it looks: an `initial_prompt` is attached to nearly every
   request, and it talks the model into scoring invented speech at 0.00.
2. **Blocklist filter** — known hallucination phrases (YouTube credits, `[Music]`,
   `[BLANK_AUDIO]`, etc.) are blocked by substring match.
3. **Repeat-loop filter** — *one* consecutive exact repeat is allowed (real
   echoed replies like "네." / "네." between speakers); the second consecutive
   repeat (< 60 chars) marks an `initial_prompt` feedback loop and is suppressed.
   A final completing a partial of the same utterance skips the check but
   **holds** the count — clearing it there let a hallucination repeat forever,
   because every utterance is a partial then a final and the reset landed
   between each pair.
4. **Decoder-loop filter** — one token repeated ≥ 6 times in a row is a broken
   decode, not speech (measured: `"너무"` ×200 in a single result). Words only,
   not characters: `하하하하하하` and `오오오오오` are real.

A blocklist cannot win on its own. Across four Korean-music sessions each
blocked phrase was simply replaced by another the next session
(`한글자막 by 한효정` → `다음 영상에서 만나요` → `감사합니다` →
`자막 제공 및 광고를 포함하고 있습니다`). Layers 3 and 4 are the ones that
generalise: they recognise the *shape* of a broken decode in any language.

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
│  ├─ subtitle-lines.ts       # line order + clipboard text, shared by view and copy
│  ├─ commands.ts             # typed wrappers over invoke()
│  └─ types.ts                # IPC types (mirrors src-tauri/src/types.rs)
└─ components/
   ├─ SubtitleView.svelte     # stacked bilingual subtitle display + per-line copy
   ├─ ControlBar.svelte       # start/stop, mode, status, settings trigger
   ├─ ProcessPicker.svelte    # per-process audio capture selector
   ├─ Icon.svelte             # the bar's stroked 16-unit icon set
   └─ SettingsPanel.svelte    # settings overlay (size, opacity, engine, providers)
```

### Copying a subtitle

Each subtitle carries its own copy button. It is the only part of the caption
tagged `data-hit`, so the bubble it sits on still lets clicks through to the
video (ADR-0012) while the button itself is reachable — the whole caption as a
hit target would hand the overlay back the clicks it exists to avoid.

Two consequences fall out of the same constraint:

- The button **cannot** be revealed on hover-of-the-caption, because the caption
  never receives the mouse. It is always present and always clickable, just
  dim until pointed at.
- Resting on it **pauses that segment's expiry**, and its eviction by the
  `MAX_SEGMENTS` cap. Without this the line disappears mid-reach: the button is
  at the edge of a bubble that is already seconds old by the time anyone
  decides to copy it. Leaving restarts the full window rather than resuming the
  remainder, which would be a fraction of a second and read as a glitch.

The clipboard write happens in Rust (`copy_to_clipboard`), not through
`navigator.clipboard`: Chromium refuses one while the document is unfocused,
which is this overlay's normal state.

### Sizing

The control bar's sizes all derive from tokens on `.bar` (`--btn`, `--fs`,
`--pad-x`, `--gap`), each of them a base value times `--ui-scale`. `--ui-scale`
is not a setting of its own — `App.svelte` derives it from the caption font
size (`fontSize / 28`, clamped 0.8–1.6) so one slider moves both. The clamp is
because the two do not share a usable range: captions work from 14 to 64 px,
but a bar at 64/28 would eat the screen and one at 14/28 would be unclickable.

`ProcessPicker` reads the same tokens by inheritance. Anything in the bar that
hardcodes a pixel size will silently stop matching the rest as soon as the
slider moves — which is exactly how the audio-source button ended up visibly
smaller than its neighbours.
