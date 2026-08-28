// IPC types — keep in sync with docs/IPC-CONTRACT.md and src-tauri/src/types.rs

/** Source language hint for Whisper. "auto" = per-chunk detection (default). */
export type SourceHint = "auto" | "zh" | "ko" | "en";

/** Target translation language. "none" = show source text only, no translation. */
export type SubtitleMode = "none" | "zh" | "ko" | "en";
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

/** A configured translation endpoint. Never carries the API key. */
export interface ProviderInfo {
  name: string;
  model: string;
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
  /** True when TRANSLATE_PROVIDERS built the list — the key/model settings below are then inert. */
  translateEnvManaged: boolean;
  openrouterModel: string;   // model slug used for translation
  openrouterKeySet: boolean; // whether a key is available; the key is never sent here
  speechThreshold: number;   // VAD RMS threshold, linear 0–1 (~0.032 = −30 dBFS)
  musicMode: boolean;
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
  fontSize: number;
  subtitleOpacity: number;
  overlay: OverlayRect;
  clickThrough: ClickThroughMode;
  /** Always returned empty — the backend never sends the stored key back. */
  openrouterApiKey: string;
  openrouterModel: string;
  speechThreshold: number;
}
