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

export interface EngineStatus {
  capture: "stopped" | "running" | "error";
  asr: "unloaded" | "loading" | "ready" | "error";
  translation: "unloaded" | "loading" | "ready" | "error";
  mode: SubtitleMode;
  sourceHint: SourceHint;
  fontSize: number;
  clickThrough: boolean;
  alwaysOnTop: boolean;
  subtitleOpacity: number;   // 0.0–1.0, controls subtitle box background alpha
  llamaGpuLayers: number;    // 0 = CPU only, 36 = full GPU
  speechThreshold: number;   // VAD RMS threshold, linear 0–1 (~0.032 = −30 dBFS)
  musicMode: boolean;
  asrBackend: string;        // "whisper" | "sensevoice"
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
  llamaGpuLayers: number;
  speechThreshold: number;
}
