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
| `set_music_mode` | `{ enabled: bool }` | `Result<()>` | Switches chunker to 10 s chunks + "Song lyrics:" prompt + beam_size=3 |
| `set_click_through` | `{ enabled: bool }` | `Result<()>` | Toggles window mouse pass-through. **Escape hatch:** `Ctrl+Alt+P` hotkey always forces OFF + re-pins on top |
| `set_always_on_top` | `{ enabled: bool }` | `Result<()>` | Re-asserts topmost; re-stacks above other topmost windows |
| `set_font_size` | `{ size: number }` | `Result<()>` | px (clamped 10–120) |
| `list_audio_processes` | — | `AudioProcess[]` | Windows processes with active audio sessions (for process picker) |
| `set_capture_process` | `{ pid: number, name: string }` | `Result<()>` | Target a specific process; `pid: 0` = system-wide loopback. Takes effect on next `start_captioning`. |
| `get_settings` | — | `PersistSettings` | For settings UI hydration |
| `update_settings` | `{ patch: SettingsPatch }` | `Result<()>` | Partial update — persisted to disk |
| `get_status` | — | `EngineStatus` | Current model/capture state |
| `dev_inject_subtitle` | `{ payload: SubtitleUpdate }` | `Result<()>` | **dev-only**; emits a real `subtitle_update` through the real event path (ADR-0005). |

## Events (Rust → frontend, via `emit`)

### `subtitle_update`

```ts
type SubtitleMode = "none" | "zh" | "ko" | "en";
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
  clickThrough: boolean;
  alwaysOnTop: boolean;
  subtitleOpacity: number;    // 0.0–1.0, subtitle box background alpha
  llamaGpuLayers: number;     // 0 = CPU, 36 = full RTX 3070
  speechThreshold: number;    // retained for API compat — no longer used (VAD removed, ADR-0009)
  musicMode: boolean;
  captureTarget?: AudioProcess; // absent/null = system-wide loopback
  rms?: number;               // present only while capturing
  message?: string;
}
```

## Types

### `SubtitleMode`

```ts
type SubtitleMode = "none" | "zh" | "ko" | "en";
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
  llamaGpuLayers: number;     // 0 = CPU, 36 = full GPU
  speechThreshold: number;    // 0 = adaptive auto-mode (recommended)
  musicMode: boolean;
}
```

### `SettingsPatch` (for `update_settings`)

All fields optional — only supplied keys are updated:

```ts
interface SettingsPatch {
  subtitleOpacity?: number;
  llamaGpuLayers?: number;
  speechThreshold?: number;   // retained for API compat — no longer used by chunker
  overlay?: { x: number; y: number; w: number; h: number };
}
```

Note: `mode`, `sourceHint`, `musicMode`, and `fontSize` have their own dedicated commands and are not part of the patch payload.
