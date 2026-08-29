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

---

## ADR-0006 — faster-whisper over whisper.cpp server

**Date:** 2026-06-08 · **Status:** Accepted · **Supersedes:** ADR-0001 (ASR half only)

**Context.** The original ADR-0001 used the official `whisper-server.exe` prebuilt
from whisper.cpp.  Two issues surfaced: (1) the C++ binary did not return
`no_speech_prob` in its `verbose_json` output, making hallucination filtering
unreliable; (2) model variants (large-v3-turbo, distil-whisper) were not available
as prebuilt Windows executables, limiting upgrade paths.

**Decision.** Switch the ASR sidecar to `asr_srv.py` — a Python `fastapi` server
supporting two pluggable backends, selected via `ASR_BACKEND` env var:
- `whisper` (default): wraps `faster-whisper` (CTranslate2). Returns `no_speech_prob`
  per segment for reliable silence/noise suppression. GPU via CUDA or CPU fallback.
- `sensevoice`: wraps SenseVoice ONNX via `sherpa-onnx`. Better Korean/multilingual
  accuracy, ~70x faster than real-time on CPU. Model ~100 MB INT8 ONNX.

Models are downloaded automatically on first run from HuggingFace.

**Consequence.** Requires Python 3.10+ and `pip install faster-whisper fastapi
uvicorn sherpa-onnx` on the target machine. The HTTP API is the same multipart
`/inference` endpoint. The Rust client lives in `asr/http_client.rs`; managed state
uses `AsrProc`. Env vars: `ASR_BACKEND`, `ASR_SERVER_SCRIPT`, `WHISPER_MODEL`,
`SENSEVOICE_MODEL`, `ASR_PORT` (legacy aliases `WHISPER_SERVER_SCRIPT`,
`WHISPER_ASR_PORT` remain accepted).

---

## ADR-0007 — SubtitleMode redesign: single-language target

**Date:** 2026-06-08 · **Status:** Accepted

**Context.** The original design used `zh-ko` / `zh-en` bilingual modes where
two languages were always shown. This created ambiguity in the translation pipeline:
the target language depended on the detected source language (e.g. zh-ko showed
Korean for Chinese input, Chinese for Korean input). This complexity was error-prone
and made it impossible to add "translate everything to Chinese regardless of source".

**Decision.** Replace the bilingual modes with a **single-target** model:
- `"zh"` — translate to Traditional Chinese (繁體中文)
- `"ko"` — translate to Korean (한국어)
- `"en"` — translate to English
- `"none"` — source text only, no translation

The target is always fixed. The source language is whatever Whisper detects. If the
source already matches the target, the translation step emits the source text directly.

**Consequence.** `SubtitleTexts` can hold all three language slots, but only the
source + target slots are populated per event. Frontend renders whatever is present.
`SourceHint` (ADR-0007b) is added separately to let users lock Whisper's detection
for single-language streams.

---

## ADR-0007b — SourceHint for Whisper language lock

**Date:** 2026-06-08 · **Status:** Accepted

**Context.** In a monolingual stream (e.g. a Korean YouTube video), Whisper's
per-chunk auto-detection occasionally misclassifies a chunk as Chinese or English,
producing garbage. Users want to lock detection without changing the translation target.

**Decision.** Add `SourceHint { Auto, Zh, Ko, En }` as a separate control. When
set to a specific code, the `language` field is sent to Whisper per request. `Auto`
(default) retains per-chunk detection.  Persisted in settings; hot-swappable.

---

## ADR-0008 — Per-process audio capture

**Date:** 2026-06-08 · **Status:** Accepted

**Context.** System-wide WASAPI loopback captures all audio — background music,
notification sounds, etc. — causing spurious subtitles. Gaming users in particular
want to caption only game dialogue or only a specific streaming app.

**Decision.** Expose Windows Process Loopback API (`ActivateAudioInterfaceAsync`
+ `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`) via `audio/process_loopback.rs`.
A `list_audio_processes` command enumerates active audio sessions; `set_capture_process`
lets the frontend target a specific PID. `pid: 0` reverts to system-wide loopback.
Requires Windows 10 Build 20348 / Windows 11.

**Consequence.** The capture path bifurcates in `audio/capture.rs` — system loopback
vs process loopback. Change takes effect on next `start_captioning` (no hot-swap
while running). COM must be initialised as MTA on a fresh thread for `list_audio_processes`
(Tauri's WebView2 STA thread is incompatible).

---

## ADR-0009 — Replace RMS VAD with fixed-chunk accumulator

**Date:** 2026-06-08 · **Status:** Accepted · **Supersedes:** M3 VAD design

**Context.** The original `pipeline/vad.rs` used an adaptive RMS threshold with
exponential noise-floor EMA and onset/silence state machine. This worked for
microphone input but proved unreliable for the primary use case — video and live
stream loopback — for two reasons:
1. Background music keeps the RMS continuously above any sensible speech threshold,
   so either speech is missed (threshold too high) or silence chunks flood ASR
   (threshold too low). No single threshold works across content types.
2. Whisper already provides `no_speech_prob` per segment, which is a superior
   silence discriminator trained on diverse audio — no RMS tuning required.

**Decision.** Delete `pipeline/vad.rs` (and `audio/ring_buffer.rs` which only it
used). Replace with `pipeline/chunker.rs`: a simple fixed-chunk accumulator that
emits 4 s chunks (64 000 samples @ 16 kHz) unconditionally. Music mode still uses
10 s chunks. Silence detection is fully delegated to Whisper's `no_speech_prob ≥ 0.7`
filter in the ASR worker.

**Consequence.** The pipeline thread structure is unchanged — a chunker worker is
still needed to buffer the 200 ms capture events into ASR-sized chunks without
blocking the capture thread. `speech_threshold` is retained in settings/state/IPC
for API compatibility but is no longer read by the chunker. Whisper may process a
slightly higher volume of chunks (a silent 4 s chunk every 4 s instead of nothing)
but `no_speech_prob` drops them cheaply before any translation is attempted.

---

## ADR-0010 — SenseVoice via sherpa-onnx for Korean ASR

**Date:** 2026-06-11 · **Status:** Accepted

**Context.** Whisper (all sizes) produces noticeably lower accuracy on Korean than
on Chinese or English, particularly for casual speech and mixed-language content.
The initial approach used `funasr` to run `FunAudioLLM/SenseVoiceSmall`, but
`funasr`'s dependency `editdistance` had no pre-built wheel for Python 3.14,
requiring a separate Python 3.12 venv.

**Decision.** Switch the SenseVoice backend to `sherpa-onnx`. Key advantages:
- `pip install sherpa-onnx` has pre-built wheels for Python 3.14 — no venv needed.
- Ships an INT8 ONNX export (~100 MB vs ~600 MB) with no PyTorch dependency.
- Structured result fields (`result.lang`, `result.event`) replace manual tag parsing.
- SenseVoice CTC runs ~70x faster than real-time on CPU; GPU not necessary.

Model: `csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17` (auto-downloaded).
Backend selected via `ASR_BACKEND=sensevoice`; whisper remains the default.

**Consequence.** `asr_srv.py` now has two inference paths behind a unified
`/inference` HTTP endpoint — Rust side unchanged. The `_sv_parse()` tag-stripping
logic from the funasr approach is replaced by reading `result.event` directly
to determine `no_speech_prob`.

---

## ADR-0011 — Hosted translation via OpenRouter, replacing local llama-server

**Date:** 2026-08-27 · **Status:** Accepted · **Supersedes:** ADR-0001 (translation half only)

**Context.** ADR-0001 chose a sidecar-first design and ran translation on a local
`llama-server` with `Qwen3-4B-Q4_K_M.gguf`. That worked, but carried real cost:
a 2.5 GB model file and ~200 MB of llama.cpp/ggml DLLs in the installer, GPU VRAM
contention with whatever the user was actually watching or playing, a CPU/GPU
offload toggle in the UI to manage that contention, and a 10–30 s model load on
first Start. The translation call itself was already OpenAI-compatible
`/v1/chat/completions`, so the local server was doing nothing that a hosted
endpoint could not.

**Decision.** Call OpenRouter's `/v1/chat/completions` directly. No child process,
no weights, no GPU offload setting. Config resolves env-var-first
(`OPENROUTER_API_KEY`, `OPENROUTER_MODEL`, `OPENROUTER_BASE_URL`) then falls back
to `settings.json`. Default model is a small fast instruct model — subtitles are
one or two sentences and latency is the binding constraint, so a larger model is
the wrong trade.

**Consequence.**
- Installer drops from ~200 MB to ~10 MB; `binaries/` and `models/` are no longer
  needed for translation.
- Translation now requires network access and an API key, and costs money per
  subtitle. The existing backlog-skipping in the translate worker matters more,
  not less: network latency is spikier than a warm local GPU.
- `LlamaProc`, `launch_llama_server`, `llama_gpu_layers` and the CPU/GPU toggle
  are all removed.
- A missing key degrades gracefully — ASR still runs and emits source-only
  subtitles, the same fallback used when an individual translation call fails.
- The API key lives in plaintext in the app data dir alongside other settings.
  It is never returned to the webview and never logged; the UI shows only a
  set/unset indicator.
- **ASR is unaffected and stays local** — OpenRouter has no speech-to-text API,
  so `asr_srv.py` and its models remain a hard requirement.

---

## ADR-0012 — Cursor-driven click-through, replacing the on/off toggle

**Date:** 2026-08-28 · **Status:** Accepted

**Context.** The overlay is a transparent, decoration-less, always-on-top window
spanning the bottom of the screen. To Windows it is a solid hit target across its
whole rectangle, so every click inside it was swallowed — including the majority
that land on empty space with no subtitle drawn under the cursor. The window
spent most of its life blocking the video it was captioning.

CSS `pointer-events: none` does not help. It decides which element inside the
page receives an event, not whether the OS delivers one to the window at all; by
the time the page could opt out, the click has already been consumed.

The existing escape valve was a manual on/off toggle backed by
`set_ignore_cursor_events`. Both of its states are wrong most of the time: "off"
blocks clicks meant for the app underneath, and "on" makes the overlay's own
controls unreachable. Users were toggling it constantly.

**Decision.** Make the policy tri-state (`off` / `auto` / `on`) and default to
`auto`, in which the flag is driven by where the cursor is. The frontend
publishes the rectangles it wants to keep clickable — the control bar, and the
settings backdrop while it is open — via `set_hit_regions`, in CSS pixels
relative to the client area. A background thread samples the global cursor every
50 ms and flips `set_ignore_cursor_events` as it crosses them.

Polling, not events: while the window is ignoring the cursor it receives no mouse
events at all, so there is no event that could tell us to turn interaction back
on. The thread is the only code allowed to call `set_ignore_cursor_events`;
commands and the tray record intent and ask for an immediate re-evaluation,
rather than writing the flag themselves and racing the poll.

**Consequence.**
- Subtitles no longer intercept clicks, which also means they can no longer be
  dragged. The control bar becomes the window's drag handle instead.
- Two failure modes are deliberately asymmetric. Reading the cursor or the
  window geometry can fail; when it does we report "over a region", leaving the
  window interactive. A window that wrongly keeps the mouse is recoverable by
  the user; one that wrongly passes it through hides its own controls.
- The flag is never flipped while the left button is down, so dragging a slider
  or the window itself cannot be dropped mid-gesture by the cursor wandering
  outside every region.
- The mode persists to `settings.json`. `Ctrl+Alt+P` still forces `off`, and the
  tray now offers all three states rather than a checkbox.
- Windows-only, like the rest of the overlay's window handling: `cursor_pos` and
  `left_button_down` are stubs elsewhere, which degrades `auto` to "always
  interactive".

---

## ADR-0013 — Provider switching from the UI

**Date:** 2026-08-28 · **Status:** Accepted · **Extends:** ADR-0011

**Context.** ADR-0011 made the translation endpoint configuration rather than
code, and that grew into an ordered list with automatic failover. But the active
index was a local variable inside the translate worker: invisible to the UI, and
changeable only by restarting the pipeline. Meanwhile the Settings panel still
offered a single key and model field that the multi-provider path never reads —
typing into them did nothing, silently.

**Decision.** Move the active index into `AppState` as a shared
`Arc<AtomicUsize>` that the worker reads before every request and writes on
failover. A `set_translate_provider` command stores into the same atomic, so a
switch from the UI lands on the next subtitle without disturbing the request in
flight. `EngineStatus` carries the key-free provider list, the active index, and
whether `TRANSLATE_PROVIDERS` built the list; the panel renders the list, marks
the live entry, and disables the key and model fields with an explanation when
they are inert.

**Consequence.**
- Failover is now visible: the UI moves its marker when the worker rotates.
- The failure counter is keyed to the provider it belongs to, so a manual switch
  does not hand the incoming provider the outgoing one's strikes.
- The list is resolved once at startup as well as per pipeline start, so
  Settings shows something useful before the first Start.
- A stale index (the list shrank between resolves) is taken modulo the length
  rather than panicking.
- Fixed alongside: `RemoteConfig::resolve` asked for `State<SettingsPath>` while
  `lib.rs` manages `State<Mutex<SettingsPath>>`. The lookup always missed, so
  the legacy branch read `PersistSettings::default()` and the key stored by the
  Settings panel was never usable at all.

---

## ADR-0014 — Settings paginates; the window tops up, it does not grow to fit

**Date:** 2026-08-28 · **Status:** Accepted

**Context.** Settings is a panel inside the subtitle overlay, not its own window.
The overlay is sized for subtitles — a couple hundred px tall by default — and
the panel had outgrown that, so it opened clipped and the user had to drag the
overlay bigger every single time to read it.

The obvious fix, growing the window to fit the panel, is the wrong one: it makes
the settings dialog's height a function of how many settings exist, so every
option added enlarges it, and it ends up swallowing the screen.

**Decision.** Paginate the panel into tabs (翻譯 / 辨識 / 外觀) so its height is
bounded by the tallest single page rather than the total. The window still tops
up on open — to a fixed 480 CSS px, once, restoring the previous geometry on
close — because the default overlay is shorter than even one page. `max-height:
calc(100vh - 60px)` with `overflow-y: auto` remains as the floor, for the cases
where the top-up cannot happen (a short screen, the clamp at the top edge).

**Consequence.**
- Adding a setting no longer changes the window size; it lands on a tab.
- The top-up grows upward so the control bar stays under the cursor that opened
  it, clamped at `y = 0`.
- The temporary geometry is not persisted: overlay move/resize saving is
  suppressed between open and close, or the overlay would come back oversized
  on the next launch.

---

## ADR-0015 — Sentence-boundary flush: the chunker cuts on text, not just time

**Date:** 2026-08-28 · **Status:** Accepted

**Context.** Measured end to end, the time from a speaker starting a sentence to
its translated subtitle appearing broke down as roughly 3–6 s of chunker wait,
320 ms of ASR, and 690 ms of translation. The API calls were ~20% of it. The
chunker dominated because it only ended an utterance two ways: a graduated
silence rule (800 → 200 ms depending on buffer length), or a 6 s hard cap. A
speaker who finishes a sentence and keeps going without a real pause pays the
full cap before anything reaches the screen.

The chunker sees only audio, so it cannot tell "finished a sentence" from "still
talking". But the partials it already sends every 1.5 s are transcribed anyway —
their punctuation is the one signal in the pipeline that knows where a sentence
ends, and reading it costs nothing.

**Decision.** Add a third flush reason. The ASR worker tests each partial's text
with `looks_complete` and, when it passes, writes that utterance id into a
shared `AtomicU64`; the chunker consumes it with `swap` and ends the utterance.
Guards, each of which cost a real mistake in testing before it was added:

- **Minimum 2 s of audio.** Whisper punctuates eagerly and a 0.4 s "네." is a
  fragment, not a sentence worth its own subtitle and translation call.
- **No ellipsis.** `...` and `…` are what Whisper writes when a speaker trails
  off mid-thought — the opposite of a boundary.
- **No period straight after a digit** — decimals and numbered list items.
- **No language flip.** Whisper hallucinates short English politeness
  ("Thank you.", "Yeah.") on a second of non-English audio, and those parse as
  complete sentences. On the Korean sample this single check separated every
  bad trigger from every good one.

**Consequence.**
- Utterances that used to hit the 6 s cap now end at 2–3 s. Measured on the
  Korean sample: 5 of 20 flushes became `[sentence]`, cutting at 2.0–3.3 s.
- Being wrong costs a subtitle split mid-sentence; being too strict costs
  nothing but the old timing. Every guard above is biased that way.
- A stale request — the ASR worker answering after the chunker already flushed
  that utterance — cannot fire, because the id no longer matches, and the same
  `swap` that checks it also clears it.
- Music mode is gone (its fixed 10 s chunks, "Song lyrics:" prompt, beam=3, and
  the lyrical translation prompt with it). It was a second segmentation policy
  competing with this one for the same code path, and unused.

---

## ADR-0016 — Start the window drag explicitly, not with `data-tauri-drag-region`

**Date:** 2026-08-28 · **Status:** Accepted

**Context.** The overlay is decoration-less, so it has no title bar; dragging
relied on `data-tauri-drag-region` on the subtitle box. That attribute never
fired in this window. Once ADR-0012 stopped the subtitle area from taking the
mouse at all, the window could not be moved by any means.

**Decision.** Call `getCurrentWindow().startDragging()` from a `pointerdown`
handler on the control bar's containers, and drop the attribute.

**Consequence.**
- Verified working by synthetic drag: the window tracks the cursor exactly.
- The bar needed a handle to grab. Its buttons had filled the row, so the
  spacer was widened, given a floor, and made visibly dotted — an unmarked gap
  between two button groups is not discoverable as a drag target.
- Explicit beats magic here anyway: the call is greppable, and it can refuse a
  press that did not land on a container.

---

## ADR-0017 — The provider list lives in `settings.json`, owned by the UI

**Date:** 2026-08-28 · **Status:** Accepted · **Supersedes:** ADR-0013 (ownership half)

**Context.** ADR-0013 let the user switch between providers but not manage them:
the list came from `TRANSLATE_PROVIDERS` and could only be changed by editing
`.env` and restarting. Adding one meant leaving the app. Worse, the panel's key
and model fields wrote to a legacy path that the env-configured list outranked,
so typing into them did nothing — the panel had two ways to configure a
provider and the visible one was the dead one.

**Decision.** `settings.json` holds the ordered list; the Settings panel owns
it. One command, `set_translate_providers`, replaces the whole thing — that is
add, remove, edit and reorder at once. The panel drives it with a form and
HTML5 drag-and-drop.

`.env` keeps working, but demoted to one job: **supplying keys**. On first run
with an empty list, `TRANSLATE_PROVIDERS` (or the older `OPENROUTER_*`) seeds
it with names, URLs and models — *not* keys, which stay in `.env` and are
resolved per call by name. So an existing setup keeps working, gains a UI, and
no secret is copied into a second file.

**Consequence.**
- Edits are live. The worker reads the list per request from a shared registry
  rather than holding a snapshot, so a reorder or a new provider lands on the
  next subtitle with no restart. The index the UI addresses can therefore never
  point at a different list than the one being called.
- The keys never leave Rust. `ProviderInfo` carries `keySource` instead, so the
  panel can show where a key came from without ever holding one, and
  `get_settings` blanks every key on the way out.
- `apiKey` in a draft is three-valued — absent keeps the stored key, `""`
  clears it, a value replaces it — because the panel cannot echo back a key it
  was never given.
- The legacy single-provider config is **migrated**, not kept as a fallback. A
  fallback that reappears after the user deletes the last entry looks like the
  delete button is broken.
- `dragDropEnabled: false` on the window: Tauri's file drag-drop handler
  otherwise swallows HTML5 drag events. The overlay accepts no file drops.
- Every tab in the panel is now the same height, so switching one does not
  resize the panel under the cursor.

## ADR-0018 — A provider's display name is not its identity

`SavedProvider.name` does three jobs at once: it keys the stored API key, it
keys `TRANSLATE_<NAME>_API_KEY`, and it is what the list shows. That makes the
obvious gesture — renaming a row because `aistudio31` reads badly — silently
orphan the key, and the panel cannot even warn usefully, because it never
receives the key it is about to lose.

So `label` was split out. `name` stays the identity and is the only thing any
lookup uses; `label` is free text with no consequences, resolved as
*typed label → preset's label → the name itself*. Presets carry a written-out
label (`groq` → `Groq`, `aistudio` → `Google AI Studio`), so an existing setup
gets readable rows without anyone editing anything.

The resolved label is what crosses IPC, so the panel has nothing to resolve.
The cost is that it must not echo a resolved label straight back on the next
edit — that would freeze today's preset text into `settings.json` and stop
future corrections from reaching an old install. `snapshot()` therefore sends
`""` whenever the label still matches the preset's.

Logs use the label too. A line reading `groq failed 2x` while the row on screen
says `Groq` is a small thing, but it is the kind of small thing that makes a
user doubt they are looking at the same provider.

## ADR-0019 — An unusable provider is shown, not dropped

`build_all` used to keep only entries it could turn into a callable provider
and log a warning about the rest. So clearing a key removed that row from
Settings while the entry stayed in `settings.json`: invisible, and therefore
impossible to either repair or delete. The same happened to any entry whose
base URL or model could not be resolved.

`build_one` now always returns a `Provider`, carrying a `Readiness` that says
what is missing (`MissingUrl` / `MissingModel` / `MissingKey`, checked in that
order — naming the key first for a row that also has nowhere to send a request
would send the user to fetch a key they still could not use). `pick_provider`
skips anything that is not `Ready`, and writes the index it landed on back to
`active`, so the "in use" badge names the provider actually being called.

That also removes a gap in the add form: it demanded an API key, which made a
provider whose key lives in `.env` as `TRANSLATE_<NAME>_API_KEY` impossible to
add through the UI. Adding without a key is now allowed, because the result is
a visible row saying 缺金鑰 rather than one that silently never appears.

The wording is duplicated on purpose: `Readiness::reason()` is English for the
logs, and the panel writes its own Chinese from the enum. One string serving
both would have to pick a language and be wrong somewhere.

## ADR-0020 — Rolling translation context expires

The worker feeds the last three (source, translation) pairs back as prior chat
turns, which is what keeps names and topic consistent across subtitles. Those
turns are re-rendered with the *current* `[source→target]` tag, so carrying
them across a language or mode change labels a Korean line as English — a
context that states something false is worse than none at all.

`context_survives` drops the history on any change of source language or
subtitle mode, and after 30 s of silence, on the grounds that a gap that long
usually means a different scene or speaker, where stale names bias the
translation rather than steady it. Extracted as a free function purely so the
rule is unit-testable; the loop it serves is not.

## ADR-0021 — `initial_prompt` disarms the no_speech filter

The first line of defence against Whisper hallucinations is
`no_speech_prob >= 0.7`: the model's own estimate that a chunk contains no
speech. Measuring it on `bench/sample.wav` showed it is far weaker in practice
than on paper.

The same 1.2 s chunk, transcribed as `Bye.` in the middle of Korean audio:

| request | initial_prompt | no_speech |
| --- | --- | --- |
| final chunk, standalone | none | **0.902** |
| same audio, as the app sends it | rolling context | **0.000** |

The prompt convinces the model that speech continues, and the score collapses.
Since a rolling prompt is attached to nearly every request, the 0.7 gate almost
never fires on exactly the chunks it exists for. Across 52 replayed requests,
four hallucinations got through — `you`, `Bye.`, `yeah`, and
`¡Bienvenidos a la secundita!` — all at 1.0–1.2 s.

Dropping the prompt is not the answer: it is what keeps names and spelling
stable across chunks. Instead, a third filter uses what stays reliable —
Whisper invents when there is too little audio to anchor it, and what it
invents tends to be in another language. **A chunk of 2 s or less whose
language differs from the established one is suppressed.** On the measured
data that is 4 of 4 caught with no false positive.

Length, not a stock-phrase blocklist: the Spanish line is in no such list, and
a switch of language worth showing runs longer than two seconds.

**Corroborated later, at scale.** A session of Korean music produced 46 lines
of memorised YouTube credits — `한글자막 by 한효정`, `다음 영상에서 만나요`,
`시청해주셔서 감사합니다` — which were the four most frequent outputs of any
kind, more than any real sentence. Every one of them scored
`no_speech_prob = 0.00`: the model was maximally certain there was speech while
transcribing something nobody said. None were caught by the 0.7 gate.

That session also showed the loop the gate cannot see. Each accepted credit
entered the rolling `initial_prompt`, which told the next chunk it was in the
outro of a video, which produced another credit at 0.00. Suppression happens
before the prompt is updated, so a blocklist entry breaks the loop as well as
hiding the line.

"Established" is a strict majority of the last five *accepted* finals, not the
previous final's language. The single-value version had a measured failure of
its own — one `Bye.` that slipped through became the reference, and the next
three correct Korean lines were then read as the language flip.

## ADR-0022 — The context note is derived from transcript, not audio

A note describing what is playing — the show, the teams, the names — measurably
helps both halves of the pipeline, and helps ASR most: a name whisper never
heard right cannot be repaired downstream, however good the translation prompt
is. But nobody types one before every stream, so an unused field would have
been the whole feature.

The obvious reading of "summarise the audio" does not work: no model in this
pipeline takes sound. The transcript is available instead, and is the better
input anyway — it is exactly what the two consumers of the note are working on,
errors and all.

So the first eight final lines are sent once to the same provider that does the
translating, asking for a short paragraph of topic and recurring proper nouns.
The answer becomes the note, rebuilt every five minutes from the most recent
forty lines so that changing video catches up within a few subtitles.

Three things this deliberately is not:

- **Not on the translate worker.** That worker is serial, so a summary call
  made there would sit in front of a subtitle. Its own thread cannot delay
  anything.
- **Not merged with a typed note.** A typed note wins outright. The user knows
  what they are watching; the summariser is inferring it from imperfect
  transcript, and two descriptions that disagree are worse guidance than
  either alone.
- **Not persisted.** It describes what is playing right now. Restoring last
  night's summary would prime tonight's session wrongly, so it is cleared when
  captioning starts.

It is shown in the Settings panel under the empty field. A note that silently
primes both models while the user cannot see it is one they cannot correct
when it is wrong — and it is built from ASR output, so sometimes it will be.

