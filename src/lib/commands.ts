// Typed wrappers over the Rust commands. See docs/IPC-CONTRACT.md.
import { invoke } from "@tauri-apps/api/core";
import type { AudioProcess, ClickThroughMode, EngineStatus, HitRect, OverlayRect, PersistSettings, ProviderDraft, SourceHint, SubtitleMode, SubtitleUpdate } from "./types";

export const startCaptioning = () => invoke<void>("start_captioning");
export const stopCaptioning = () => invoke<void>("stop_captioning");

export const setSubtitleMode = (mode: SubtitleMode) =>
  invoke<void>("set_subtitle_mode", { mode });

export const setSourceHint = (hint: SourceHint) =>
  invoke<void>("set_source_hint", { hint });

export const setClickThrough = (mode: ClickThroughMode) =>
  invoke<void>("set_click_through", { mode });

/**
 * Publish the rectangles that must stay clickable while the window is in
 * `auto` mode. Anywhere else, the mouse goes to whatever is behind the overlay.
 *
 * Coordinates are CSS pixels relative to the window's client area — exactly
 * what `getBoundingClientRect()` returns; Rust converts to screen coordinates.
 * Send the full set every time: this replaces the previous one.
 */
export const setHitRegions = (regions: HitRect[]) =>
  invoke<void>("set_hit_regions", { regions });

/**
 * Put `text` on the system clipboard.
 *
 * Rust does the write, not `navigator.clipboard`: Chromium refuses one while
 * the document is unfocused, which is the normal state for this overlay.
 */
export const copyToClipboard = (text: string) =>
  invoke<void>("copy_to_clipboard", { text });

/** Switch the active translation provider. Takes effect on the next subtitle. */
export const setTranslateProvider = (index: number) =>
  invoke<void>("set_translate_provider", { index });

/**
 * Replace the whole provider list — add, remove, edit and reorder in one call,
 * because the panel owns the order so every edit is "here is the new list".
 * Takes effect on the next subtitle; no restart.
 */
export const setTranslateProviders = (providers: ProviderDraft[]) =>
  invoke<void>("set_translate_providers", { providers });

/** Names with a built-in base URL and model, so the add form can ask only for a key. */
export const translatePresetNames = () =>
  invoke<{ name: string; label: string }[]>("translate_preset_names");

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
  asrBackend?: string;
  whisperModel?: string;
  sensevoicePrecision?: string;
  speechThreshold?: number;
  overlay?: OverlayRect;
}

export const updateSettings = (patch: SettingsPatch) =>
  invoke<void>("update_settings", { patch });

// ── first-run setup ───────────────────────────────────────────────────────────

export interface SetupState {
  ready: boolean;
  envRoot: string;
  /** False in a build shipped without `uv.exe`; the manual route is all there is. */
  canInstall: boolean;
}

export interface SetupProgress {
  line: string;
  done: boolean;
  ok: boolean;
  message: string;
}

/** Whether the ASR sidecar has a Python environment to run in. */
export const getSetupState = () => invoke<SetupState>("get_setup_state");

/**
 * Build that environment. Returns as soon as the work is handed to a thread;
 * watch the `setup_progress` event for what happens next.
 */
export const runAsrSetup = () => invoke<void>("run_asr_setup");

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
