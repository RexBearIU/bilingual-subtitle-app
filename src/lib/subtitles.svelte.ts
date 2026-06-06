// Event bridge + reactive overlay state (Svelte 5 runes).
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { EngineStatus, SubtitleUpdate } from "./types";

class OverlayStore {
  /** The subtitle currently shown (latest update for the active segment). */
  current = $state<SubtitleUpdate | null>(null);
  /** Latest engine/UI status from the backend. */
  status = $state<EngineStatus | null>(null);

  private unlisten: UnlistenFn[] = [];

  async connect(): Promise<void> {
    this.unlisten.push(
      await listen<SubtitleUpdate>("subtitle_update", (e) => {
        // M6 will add dedup/merge/expire; M1 just shows the latest.
        this.current = e.payload;
      }),
    );
    this.unlisten.push(
      await listen<EngineStatus>("engine_status", (e) => {
        this.status = e.payload;
      }),
    );
  }

  disconnect(): void {
    for (const un of this.unlisten) un();
    this.unlisten = [];
  }
}

export const overlay = new OverlayStore();
