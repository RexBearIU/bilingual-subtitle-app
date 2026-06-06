// Typed wrappers over the Rust commands. See docs/IPC-CONTRACT.md.
import { invoke } from "@tauri-apps/api/core";
import type { EngineStatus, SubtitleMode, SubtitleUpdate } from "./types";

export const startCaptioning = () => invoke<void>("start_captioning");
export const stopCaptioning = () => invoke<void>("stop_captioning");

export const setSubtitleMode = (mode: SubtitleMode) =>
  invoke<void>("set_subtitle_mode", { mode });

export const setClickThrough = (enabled: boolean) =>
  invoke<void>("set_click_through", { enabled });

/** Re-pin the overlay to the top of the always-on-top band. */
export const setAlwaysOnTop = (enabled: boolean) =>
  invoke<void>("set_always_on_top", { enabled });

export const setFontSize = (size: number) =>
  invoke<void>("set_font_size", { size });

export const getStatus = () => invoke<EngineStatus>("get_status");

/** dev-only — emits a real `subtitle_update` (ADR-0005). */
export const devInjectSubtitle = (payload: SubtitleUpdate) =>
  invoke<void>("dev_inject_subtitle", { payload });
