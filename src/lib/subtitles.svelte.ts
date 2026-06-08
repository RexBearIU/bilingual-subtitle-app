// Event bridge + reactive subtitle state (Svelte 5 runes).
//
// M6 subtitle state manager:
//   • dedup      — subtitle_update with same `id` replaces the existing slot
//   • merge      — partial (is_final=false) updated in-place when final arrives
//   • expire     — segments disappear EXPIRE_MS after becoming final
//   • max cap    — never show more than MAX_SEGMENTS at once

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { EngineStatus, SubtitleUpdate } from "./types";

/** How long a final segment stays on screen (ms).
 *  8 s gives enough time to read long-paragraph chunks before they expire. */
const EXPIRE_MS = 8_000;
/** Maximum simultaneous segments shown. */
const MAX_SEGMENTS = 4;

class OverlayStore {
  /** Active subtitle segments — oldest first, newest last. */
  segments = $state<SubtitleUpdate[]>([]);
  /** Latest engine/UI status from the backend. */
  status = $state<EngineStatus | null>(null);

  // Internal: tracks expiry timestamp per segment id.
  // null  = still partial (no expiry yet)
  // number = Unix timestamp (ms) after which the segment should be pruned
  private _expiry = new Map<string, number | null>();

  private _unlisten: UnlistenFn[] = [];
  private _timer: ReturnType<typeof setInterval> | null = null;

  async connect(): Promise<void> {
    this._unlisten.push(
      await listen<SubtitleUpdate>("subtitle_update", (e) => {
        this._handleUpdate(e.payload);
      }),
    );
    this._unlisten.push(
      await listen<EngineStatus>("engine_status", (e) => {
        this.status = e.payload;
      }),
    );
    // Prune expired segments every 500 ms.
    this._timer = setInterval(() => this._prune(), 500);
  }

  disconnect(): void {
    for (const un of this._unlisten) un();
    this._unlisten = [];
    if (this._timer !== null) {
      clearInterval(this._timer);
      this._timer = null;
    }
    this.segments = [];
    this._expiry.clear();
  }

  // ── private ──────────────────────────────────────────────────────────────

  private _handleUpdate(update: SubtitleUpdate): void {
    const now = Date.now();
    const idx = this.segments.findIndex((s) => s.id === update.id);

    if (idx >= 0) {
      // Replace in-place: partial → final or partial → partial.
      this.segments[idx] = update;
      if (update.isFinal) {
        this._expiry.set(update.id, now + EXPIRE_MS);
      }
    } else {
      // Brand-new segment.
      this.segments.push(update);
      this._expiry.set(update.id, update.isFinal ? now + EXPIRE_MS : null);

      // Drop the oldest segment(s) if we're over the cap.
      if (this.segments.length > MAX_SEGMENTS) {
        const removed = this.segments.splice(0, this.segments.length - MAX_SEGMENTS);
        for (const r of removed) this._expiry.delete(r.id);
      }
    }
  }

  private _prune(): void {
    const now = Date.now();
    const before = this.segments.length;
    const keep = this.segments.filter((s) => {
      const exp = this._expiry.get(s.id);
      return exp === undefined || exp === null || exp > now;
    });
    if (keep.length !== before) {
      // Clean up expiry map for removed entries.
      for (const s of this.segments) {
        if (!keep.includes(s)) this._expiry.delete(s.id);
      }
      this.segments = keep;
    }
  }
}

export const overlay = new OverlayStore();
