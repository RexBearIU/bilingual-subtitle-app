# IPC Contract (Frontend ↔ Backend)

The frontend is **render-only**: it sends commands and renders events. All heavy
work and all model access live in Rust. This contract is the stable boundary;
keep it in sync with `src-tauri/src/commands.rs` and `src/lib/commands.ts`.

## Commands (frontend → Rust, via `invoke`)

| Command | Args | Returns | Notes |
|---------|------|---------|-------|
| `start_captioning` | — | `Result<()>` | Starts capture→ASR→translate pipeline |
| `stop_captioning` | — | `Result<()>` | Stops pipeline; sidecars stay resident |
| `set_subtitle_mode` | `{ mode: "zh-ko" \| "zh-en" }` | `Result<()>` | Hot-swappable while running |
| `set_click_through` | `{ enabled: bool }` | `Result<()>` | Toggles window mouse pass-through. **Escape hatch:** global hotkey `Ctrl+Alt+P` always forces this OFF + re-pins on top (recovers from lockout) |
| `set_always_on_top` | `{ enabled: bool }` | `Result<()>` | Re-asserts topmost; re-stacks above other topmost windows |
| `set_font_size` | `{ size: number }` | `Result<()>` | px |
| `get_settings` | — | `Settings` | For settings UI hydration |
| `update_settings` | `{ patch: Partial<Settings> }` | `Result<()>` | Persisted (M7) |
| `get_status` | — | `EngineStatus` | Model/sidecar/capture state |
| `dev_inject_subtitle` | `{ payload: SubtitleUpdate }` | `Result<()>` | **dev-only**; emits a real `subtitle_update`. Replaces "mock" — same event path, manual source. Removed/feature-gated for release. |

## Events (Rust → frontend, via `emit`)

### `subtitle_update`

```ts
interface SubtitleUpdate {
  sourceLang: "ko" | "en" | "zh";
  sourceText: string;
  mode: "zh-ko" | "zh-en";
  subtitles: {            // only the two languages for the active mode are populated
    zh?: string;
    ko?: string;
    en?: string;
  };
  isFinal: boolean;       // partial (streaming) vs finalized segment
  id: string;             // stable per segment, for dedup/replace on the UI
  startedAtMs?: number;
  endedAtMs?: number;
}
```

Example:

```json
{
  "id": "seg_0192",
  "sourceLang": "ko",
  "sourceText": "오늘 진짜 재밌네요",
  "mode": "zh-en",
  "subtitles": { "zh": "今天真的很好玩。", "en": "This is really fun today." },
  "isFinal": true
}
```

### `engine_status` (M4+)

```ts
interface EngineStatus {
  capture: "stopped" | "running" | "error";
  asr: "unloaded" | "loading" | "ready" | "error";
  translation: "unloaded" | "loading" | "ready" | "error";
  rms?: number;           // debug meter (M2)
  message?: string;
}
```

## Settings shape

```ts
interface Settings {
  mode: "zh-ko" | "zh-en";
  fontSize: number;
  maxLines: number;
  opacity: number;        // 0..1
  clickThrough: boolean;
  overlay: { x: number; y: number; w: number; h: number };
  asrModelPath: string;
  translationModelPath: string;
  latencyMode: "low-latency" | "high-quality";
  asrBackend: "whisper.cpp" | "sensevoice";   // M9
}
```
