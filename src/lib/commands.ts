// Typed wrappers over the Rust commands. See docs/IPC-CONTRACT.md.
import { invoke } from "@tauri-apps/api/core";
import type { AudioProcess, EngineStatus, OverlayRect, PersistSettings, SourceHint, SubtitleMode, SubtitleUpdate } from "./types";

export const startCaptioning = () => invoke<void>("start_captioning");
export const stopCaptioning = () => invoke<void>("stop_captioning");

export const setSubtitleMode = (mode: SubtitleMode) =>
  invoke<void>("set_subtitle_mode", { mode });

export const setSourceHint = (hint: SourceHint) =>
  invoke<void>("set_source_hint", { hint });

export const setMusicMode = (enabled: boolean) =>
  invoke<void>("set_music_mode", { enabled });

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

// ── settings ─────────────────────────────────────────────────────────────────

export const getSettings = () => invoke<PersistSettings>("get_settings");

export interface SettingsPatch {
  subtitleOpacity?: number;
  llamaGpuLayers?: number;
  speechThreshold?: number;
  overlay?: OverlayRect;
}

export const updateSettings = (patch: SettingsPatch) =>
  invoke<void>("update_settings", { patch });

// ── process capture ───────────────────────────────────────────────────────────

/** Return all processes that currently have an active audio session. */
export const listAudioProcesses = () =>
  invoke<AudioProcess[]>("list_audio_processes");

/**
 * Set the per-process capture target.
 * Pass `pid: 0` to revert to system-wide loopback.
 * Change takes effect on the next `startCaptioning()` call.
 */
export const setCaptureProcess = (pid: number, name: string) =>
  invoke<void>("set_capture_process", { pid, name });
