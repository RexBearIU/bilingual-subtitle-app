// IPC types — keep in sync with docs/IPC-CONTRACT.md and src-tauri/src/types.rs

/** Source language hint for Whisper. "auto" = per-chunk detection (default). */
export type SourceHint = "auto" | "zh" | "ko" | "en";

/** Target translation language. "none" = show source text only, no translation. */
/** `zh` is Traditional — the name it had before Simplified was added. */
export type SubtitleMode = "none" | "zh" | "zh-hans" | "ko" | "en";
export type SourceLang = "ko" | "en" | "zh";
export type Lang = "zh" | "ko" | "en";

export interface SubtitleTexts {
  zh?: string;
  ko?: string;
  en?: string;
}

export interface SubtitleUpdate {
  id: string;
  sourceLang: SourceLang;
  sourceText: string;
  mode: SubtitleMode;
  subtitles: SubtitleTexts;
  isFinal: boolean;
  startedAtMs?: number;
  endedAtMs?: number;
}

export interface AudioProcess {
  pid: number;
  name: string;
}

/**
 * How the overlay window treats the mouse.
 * - `off`  — the whole window takes the mouse, empty areas included.
 * - `auto` — passes through except over the regions reported by `setHitRegions`.
 * - `on`   — nothing is clickable; the mouse always goes to what is behind.
 */
export type ClickThroughMode = "off" | "auto" | "on";

/** Where a provider's API key was found. */
export type KeySource = "settings" | "env";

/** Whether a provider can actually be called, and if not, why. */
export type Readiness = "ready" | "missingKey" | "missingUrl" | "missingModel";

/** A configured translation endpoint. Never carries the API key. */
export interface ProviderInfo {
  /** The identity: keys the stored API key and TRANSLATE_<NAME>_API_KEY. */
  name: string;
  /** What to show. Already resolved: the preset's label, else the name. */
  label: string;
  model: string;
  baseUrl: string;
  /** `env` = supplied by TRANSLATE_<NAME>_API_KEY rather than typed in Settings. */
  keySource: KeySource;
  /** Anything but `ready` means this entry is shown but skipped when translating. */
  readiness: Readiness;
}

/**
 * One entry as the Settings panel sends it back.
 *
 * `apiKey` is three-valued because the panel never receives the stored key:
 * omit it to keep what is stored, `""` to clear it (the environment then takes
 * over again), or a value to replace it.
 */
export interface ProviderDraft {
  name: string;
  /** "" = use the preset's label, else the name. */
  label: string;
  baseUrl: string;
  model: string;
  apiKey?: string;
}

/** A rectangle that must stay clickable, in CSS px relative to the client area. */
export interface HitRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface EngineStatus {
  capture: "stopped" | "running" | "error";
  asr: "unloaded" | "loading" | "ready" | "error";
  translation: "unloaded" | "loading" | "ready" | "error";
  mode: SubtitleMode;
  sourceHint: SourceHint;
  fontSize: number;
  clickThrough: ClickThroughMode;
  /** Whether the mouse is passing through right now (flips as the cursor moves in `auto`). */
  clickThroughActive: boolean;
  alwaysOnTop: boolean;
  subtitleOpacity: number;   // 0.0–1.0, controls subtitle box background alpha
  /** Configured providers in preference order; index 0 is tried first. */
  translateProviders: ProviderInfo[];
  /** Index into `translateProviders` currently in use. */
  translateActive: number;
  speechThreshold: number;   // VAD RMS threshold, linear 0–1 (~0.032 = −30 dBFS)
  asrBackend: string;        // "whisper" | "sensevoice" | "zipformer-ko"
  whisperModel: string;      // "turbo" | "large"
  sensevoicePrecision: string; // "int8" | "fp32"
  captureTarget?: AudioProcess; // null / absent = system-wide loopback
  rms?: number;
  message?: string;
}

export interface OverlayRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface PersistSettings {
  mode: SubtitleMode;
  sourceHint: SourceHint;
  /** What the audio is about, in the user's words. Primes ASR and translation. */
  context: string;
  fontSize: number;
  subtitleOpacity: number;
  overlay: OverlayRect;
  clickThrough: ClickThroughMode;
  /** Legacy, always returned empty; superseded by the provider list. */
  openrouterApiKey: string;
  openrouterModel: string;
  speechThreshold: number;
}
