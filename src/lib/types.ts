// IPC types — keep in sync with docs/IPC-CONTRACT.md and src-tauri/src/types.rs

export type SubtitleMode = "zh-ko" | "zh-en";
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

export interface EngineStatus {
  capture: "stopped" | "running" | "error";
  asr: "unloaded" | "loading" | "ready" | "error";
  translation: "unloaded" | "loading" | "ready" | "error";
  mode: SubtitleMode;
  fontSize: number;
  clickThrough: boolean;
  alwaysOnTop: boolean;
  rms?: number;
  message?: string;
}
