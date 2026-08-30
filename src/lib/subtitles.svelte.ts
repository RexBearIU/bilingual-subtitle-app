// Event bridge + reactive subtitle state (Svelte 5 runes).
//
// M6 subtitle state manager:
//   • dedup      — subtitle_update with same `id` replaces the existing slot
//   • merge      — partial (is_final=false) updated in-place when final arrives
//   • expire     — segments disappear EXPIRE_MS after becoming final
//   • max cap    — never show more than MAX_SEGMENTS at once

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { copyToClipboard } from "./commands";
import { asClipboardText } from "./subtitle-lines";
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
  /** Id of the segment copied a moment ago, so its button can acknowledge it. */
  copiedId = $state<string | null>(null);
  /**
   * Transient "copied" note, shown as a toast over the subtitles.
   *
   * The button turning green is enough when you clicked the button. The hotkey
   * is the case that needs this: it is used precisely while looking at the
   * video rather than at the overlay, and a 1.5em button changing colour is
   * not an answer to "did that work?".
   */
  copiedNote = $state<string | null>(null);

  // Internal: tracks expiry timestamp per segment id.
  // null  = still partial (no expiry yet)
  // number = Unix timestamp (ms) after which the segment should be pruned
  private _expiry = new Map<string, number | null>();

  private _unlisten: UnlistenFn[] = [];
  private _timer: ReturnType<typeof setInterval> | null = null;
  private _copiedTimer: ReturnType<typeof setTimeout> | null = null;
  /** Segment the cursor is currently resting on; exempt from expiry and eviction. */
  private _heldId: string | null = null;

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

  /**
   * Keep `id` on screen while the cursor rests on its copy button, and release
   * it with a fresh reading window afterwards.
   *
   * Without this the segment expires out from under the pointer mid-reach: the
   * button is at the right edge of a bubble that is already several seconds
   * old by the time anyone decides to copy it. Passing `null` releases.
   */
  hold(id: string | null): void {
    if (id === null && this._heldId !== null) {
      // Restart the clock rather than resume it. Whatever was left of the
      // original window is likely to be a fraction of a second, and having the
      // line disappear the instant the cursor leaves reads as a glitch.
      this._expiry.set(this._heldId, Date.now() + EXPIRE_MS);
    }
    this._heldId = id;
  }

  /**
   * Copy one segment — every line of it, in the order it is displayed.
   *
   * This is what the per-subtitle button calls. Copying a single line is the
   * common want: the sentence you just heard and did not catch.
   */
  async copySegment(id: string): Promise<boolean> {
    const seg = this.segments.find((s) => s.id === id);
    if (!seg) return false;
    return this._copy(asClipboardText([seg]), id, "已複製");
  }

  private async _copy(text: string, ackId: string | null, note: string): Promise<boolean> {
    // An empty overlay is not worth clearing the user's clipboard for.
    if (!text) return false;
    try {
      await copyToClipboard(text);
    } catch (e) {
      console.warn("copy failed", e);
      return false;
    }
    this.copiedId = ackId;
    this.copiedNote = note;
    if (this._copiedTimer !== null) clearTimeout(this._copiedTimer);
    this._copiedTimer = setTimeout(() => {
      this.copiedId = null;
      this.copiedNote = null;
    }, 1400);
    return true;
  }

  disconnect(): void {
    for (const un of this._unlisten) un();
    this._unlisten = [];
    if (this._timer !== null) {
      clearInterval(this._timer);
      this._timer = null;
    }
    if (this._copiedTimer !== null) {
      clearTimeout(this._copiedTimer);
      this._copiedTimer = null;
    }
    this.copiedId = null;
    this.copiedNote = null;
    this._heldId = null;
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

      // Drop the oldest segment(s) if we're over the cap — except one the
      // cursor is resting on, which would otherwise be yanked away by the
      // arrival of a new subtitle rather than by its own expiry.
      while (this.segments.length > MAX_SEGMENTS) {
        const victim = this.segments.findIndex((s) => s.id !== this._heldId);
        if (victim < 0) break;
        const [r] = this.segments.splice(victim, 1);
        this._expiry.delete(r.id);
      }
    }
  }

  private _prune(): void {
    const now = Date.now();
    const before = this.segments.length;
    const keep = this.segments.filter((s) => {
      if (s.id === this._heldId) return true;
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
