# IPC Contract (Frontend ↔ Backend)

The frontend is **render-only**: it sends commands and renders events. All heavy
work and all model access live in Rust. This contract is the stable boundary;
keep it in sync with `src-tauri/src/commands.rs`, `src-tauri/src/types.rs`,
and `src/lib/commands.ts` / `src/lib/types.ts`.

## Commands (frontend → Rust, via `invoke`)

| Command | Args | Returns | Notes |
|---------|------|---------|-------|
| `start_captioning` | — | `Result<()>` | Starts capture→VAD→ASR→translate pipeline; launches sidecars if not running |
| `stop_captioning` | — | `Result<()>` | Stops pipeline; sidecars stay resident (models stay loaded) |
| `set_subtitle_mode` | `{ mode: SubtitleMode }` | `Result<()>` | Hot-swappable while running |
| `set_source_hint` | `{ hint: SourceHint }` | `Result<()>` | Language hint passed to Whisper per chunk |
| `set_click_through` | `{ mode: ClickThroughMode }` | `Result<()>` | Window mouse policy. **Escape hatch:** `Ctrl+Alt+P` always forces `"off"` + re-pins on top |
| `set_hit_regions` | `{ regions: HitRect[] }` | `Result<()>` | Rectangles that stay clickable in `"auto"`. CSS px relative to the client area; replaces the previous set |
| `set_translate_provider` | `{ index: number }` | `Result<()>` | Switch the active provider; takes effect on the next subtitle. Index into `EngineStatus.translateProviders` |
| `set_translate_providers` | `{ providers: ProviderDraft[] }` | `Result<()>` | Replace the whole list — add, remove, edit and reorder in one call. Persisted to `settings.json` |
| `translate_preset_names` | — | `{ name, label }[]` | Providers with a built-in base URL, model and display label, so the add form can ask only for a key |
| `set_always_on_top` | `{ enabled: bool }` | `Result<()>` | Re-asserts topmost; re-stacks above other topmost windows |
| `set_font_size` | `{ size: number }` | `Result<()>` | px (clamped 10–120). Also scales the control bar — see ARCHITECTURE.md §Sizing |
| `copy_to_clipboard` | `{ text: string }` | `Result<()>` | Written from Rust: Chromium refuses `navigator.clipboard` while the document is unfocused, which is this overlay's normal state |
| `list_audio_processes` | — | `AudioProcess[]` | Windows processes with active audio sessions (for process picker) |
| `set_capture_process` | `{ pid: number, name: string }` | `Result<()>` | Target a specific process; `pid: 0` = system-wide loopback. Takes effect on next `start_captioning`. |
| `get_settings` | — | `PersistSettings` | For settings UI hydration |
| `update_settings` | `{ patch: SettingsPatch }` | `Result<()>` | Partial update — persisted to disk |
| `get_status` | — | `EngineStatus` | Current model/capture state |
| `dev_inject_subtitle` | `{ payload: SubtitleUpdate }` | `Result<()>` | **dev-only**; emits a real `subtitle_update` through the real event path (ADR-0005). |

## Events (Rust → frontend, via `emit`)

### `subtitle_update`

```ts
// `zh` is Traditional — the name it carried before Simplified was added,
// kept so an existing settings.json still selects what it always did.
type SubtitleMode = "none" | "zh" | "zh-hans" | "ko" | "en";
type SourceLang   = "ko" | "en" | "zh";

interface SubtitleTexts {
  zh?: string;
  ko?: string;
  en?: string;
}

interface SubtitleUpdate {
  id: string;             // stable per utterance — partial & final share the same id
  sourceLang: SourceLang; // detected source language
  sourceText: string;     // raw ASR text
  mode: SubtitleMode;     // active translation mode at time of emission
  subtitles: SubtitleTexts; // only the source slot (isFinal=false) or source+target (isFinal=true)
  isFinal: boolean;       // false = partial (source only); true = translation complete
  startedAtMs?: number;
  endedAtMs?: number;
}
```

**Two-phase emission per utterance:**
1. On ASR completion → `isFinal: false`, only source slot populated (e.g. `subtitles.ko`)
2. On translation completion → same `id`, `isFinal: true`, target slot added (e.g. `subtitles.zh`)

Example:

```json
{
  "id": "asr_7",
  "sourceLang": "ko",
  "sourceText": "오늘 진짜 재밌네요",
  "mode": "zh",
  "subtitles": { "ko": "오늘 진짜 재밌네요", "zh": "今天真的很好玩。" },
  "isFinal": true
}
```

### `engine_status`

```ts
type SourceHint = "auto" | "zh" | "ko" | "en";

interface AudioProcess {
  pid: number;
  name: string;   // e.g. "chrome.exe"
}

interface EngineStatus {
  capture: "stopped" | "running" | "error";
  asr: "unloaded" | "loading" | "ready" | "error";
  translation: "unloaded" | "loading" | "ready" | "error";
  mode: SubtitleMode;
  sourceHint: SourceHint;
  fontSize: number;
  clickThrough: ClickThroughMode;
  clickThroughActive: boolean; // whether the mouse is passing through right now
  alwaysOnTop: boolean;
  subtitleOpacity: number;    // 0.0–1.0, subtitle box background alpha
  /** What the summariser inferred from the transcript. Empty when a note
   *  was typed by hand, which wins outright. Never persisted. */
  autoContext: string;
  translateProviders: ProviderInfo[]; // preference order; index 0 is tried first
  translateActive: number;    // index currently in use (moves on failover too)
  speechThreshold: number;    // retained for API compat — no longer used (VAD removed, ADR-0009)
  asrBackend: string;         // "whisper" | "sensevoice" | "zipformer-ko"
  whisperModel: string;       // "turbo" | "large" (large-v3 int8_float16)
  sensevoicePrecision: string;// "int8" | "fp32"
  captureTarget?: AudioProcess; // absent/null = system-wide loopback
  rms?: number;               // present only while capturing
  message?: string;           // last process-loopback error (shown by ProcessPicker)
}

```ts
/**
 * How the overlay window treats the mouse.
 * - `off`  — the whole window takes the mouse, empty areas included.
 * - `auto` — passes through except over the regions from `set_hit_regions`.
 * - `on`   — nothing is clickable; the mouse always goes behind.
 *
 * `auto` is the default. A transparent, decoration-less window is a solid
 * hit target to the OS, so without it an empty overlay blocks whatever is
 * playing underneath. CSS `pointer-events` cannot fix this — it decides
 * which element gets an event, not whether the window receives one.
 */
type ClickThroughMode = "off" | "auto" | "on";

/** A clickable rectangle, CSS px relative to the window client area. */
interface HitRect { x: number; y: number; w: number; h: number }

/** A configured translation endpoint. Never carries the API key. */
interface ProviderInfo {
  /** The identity: keys the stored API key and TRANSLATE_<NAME>_API_KEY. */
  name: string;
  /** Display text, already resolved: the preset's label, else `name`. */
  label: string;
  model: string;
  baseUrl: string;
  /** `env` = the key came from TRANSLATE_<NAME>_API_KEY, not from Settings. */
  keySource: "settings" | "env";
  /**
   * Whether this entry can be called. Anything but `ready` is still listed —
   * it is shown so it can be fixed or deleted — but is skipped when
   * translating, and never appears as `translateActive`.
   */
  readiness: "ready" | "missingKey" | "missingUrl" | "missingModel";
}

/**
 * One entry as the Settings panel sends it back.
 *
 * `apiKey` is three-valued because the panel never receives the stored key:
 * omit it to keep what is stored, `""` to clear it (the environment then takes
 * over again), or a value to replace it.
 */
interface ProviderDraft {
  name: string;
  label: string;     // "" = use the built-in preset's label, else `name`
  baseUrl: string;   // "" = use the built-in preset for this name
  model: string;     // "" = use the built-in preset for this name
  apiKey?: string;
}
```
```

## Types

### `SubtitleMode`

```ts
// `zh` is Traditional — the name it carried before Simplified was added,
// kept so an existing settings.json still selects what it always did.
type SubtitleMode = "none" | "zh" | "zh-hans" | "ko" | "en";
```

- `"none"` — show source text only, no translation
- `"zh"` — translate everything to Traditional Chinese (繁體中文)
- `"ko"` — translate everything to Korean (한국어)
- `"en"` — translate everything to English

### `SourceHint`

```ts
type SourceHint = "auto" | "zh" | "ko" | "en";
```

Passed to Whisper as the `language` field. `"auto"` = per-chunk detection (default, best for mixed-language streams).

## Settings shape

```ts
interface PersistSettings {
  mode: SubtitleMode;
  sourceHint: SourceHint;
  fontSize: number;
  subtitleOpacity: number;    // 0.0–1.0
  overlay: { x: number; y: number; w: number; h: number };
  clickThrough: ClickThroughMode;
  /** The ordered provider list. Every `apiKey` is ALWAYS returned as "". */
  /** What this audio is about, in the user's words. "" = derive it. */
  context: string;
  providers: { name: string; label: string; baseUrl: string; apiKey: string; model: string }[];
  openrouterApiKey: string;   // legacy, ALWAYS ""; migrated into `providers` on first launch
  openrouterModel: string;    // legacy, superseded by `providers`
  speechThreshold: number;    // 0 = adaptive auto-mode (recommended)
  asrBackend: string;         // "whisper" | "sensevoice" | "zipformer-ko"
  whisperModel: string;       // "turbo" | "large"
  sensevoicePrecision: string;// "int8" | "fp32"
}
```

### `SettingsPatch` (for `update_settings`)

All fields optional — only supplied keys are updated:

```ts
interface SettingsPatch {
  subtitleOpacity?: number;
  asrBackend?: string;        // kills idle asr-srv so next Start relaunches with the new backend
  whisperModel?: string;      // "turbo" | "large" — same relaunch behavior
  sensevoicePrecision?: string; // "int8" | "fp32" — same relaunch behavior
  speechThreshold?: number;   // retained for API compat — no longer used by chunker
  overlay?: { x: number; y: number; w: number; h: number };
}
```

Note: `mode`, `sourceHint`, and `fontSize` have their own dedicated commands and are not part of the patch payload.
