<script lang="ts">
  import { onMount } from "svelte";
  import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import ControlBar from "./components/ControlBar.svelte";
  import SettingsPanel from "./components/SettingsPanel.svelte";
  import SubtitleView from "./components/SubtitleView.svelte";
  import { overlay } from "./lib/subtitles.svelte";
  import { getStatus, setHitRegions, updateSettings } from "./lib/commands";
  import type { HitRect } from "./lib/types";

  let settingsOpen  = $state(false);
  let subsHidden    = $state(false);   // subtitle visibility toggle

  // The overlay is sized for subtitles, which can be shorter than the settings
  // panel — so opening Settings used to show a panel clipped by the window,
  // which the user had to resize by hand to read. The panel paginates itself
  // into tabs; this only tops the window up to the one tab-page it needs, and
  // puts the old geometry back on close. Deliberately modest: a settings dialog
  // that grows to swallow the screen is its own problem.
  //
  // CSS pixels, compared against `window.innerHeight` rather than the window's
  // reported size: that is the exact unit the panel's own `100vh` is measured
  // in, so there is no scale factor to get backwards.
  const SETTINGS_MIN_HEIGHT = 480;

  /** Geometry to put back when Settings closes; null when we did not resize. */
  let restoreGeom: { size: PhysicalSize; pos: PhysicalPosition } | null = null;
  /** Suppresses move/resize persistence while Settings drives the window. */
  let suppressSave = false;

  onMount(() => {
    let disconnected = false;

    (async () => {
      await overlay.connect();

      try { overlay.status = await getStatus(); }
      catch (e) { console.error("getStatus failed", e); }

      if (disconnected) { overlay.disconnect(); return; }

      const appWindow = getCurrentWindow();
      let saveTimer: ReturnType<typeof setTimeout> | null = null;

      async function saveOverlay() {
        try {
          const [pos, size] = await Promise.all([
            appWindow.outerPosition(),
            appWindow.outerSize(),
          ]);
          await updateSettings({ overlay: { x: pos.x, y: pos.y, w: size.width, h: size.height } });
        } catch (e) { console.warn("saveOverlay failed", e); }
      }

      function scheduleOverlaySave() {
        // A Settings-driven resize is temporary; persisting it would bring the
        // overlay back oversized on the next launch.
        if (suppressSave) return;
        if (saveTimer !== null) clearTimeout(saveTimer);
        saveTimer = setTimeout(saveOverlay, 400);
      }

      const unlistenMove   = await appWindow.onMoved(scheduleOverlaySave);
      const unlistenResize = await appWindow.onResized(scheduleOverlaySave);

      if (disconnected) {
        overlay.disconnect(); unlistenMove(); unlistenResize(); return;
      }

      return () => {
        disconnected = true;
        overlay.disconnect(); unlistenMove(); unlistenResize();
        if (saveTimer !== null) clearTimeout(saveTimer);
      };
    })();
  });

  let fontSize     = $derived(overlay.status?.fontSize        ?? 28);
  let clickThrough = $derived(overlay.status?.clickThrough    ?? "auto");
  let opacity      = $derived(overlay.status?.subtitleOpacity ?? 0.55);

  // Controls stay interactive except in full click-through mode.
  // We do not rely on mouseenter/leave (unreliable on Tauri transparent windows).
  let showControls = $derived(clickThrough !== "on");

  // ── click-through hit regions ──────────────────────────────────────────────
  //
  // In `auto` the window passes the mouse through everywhere except the
  // rectangles reported here, so an empty overlay stops blocking whatever is
  // playing behind it. Elements opt in with `data-hit`; anything untagged —
  // the subtitles themselves — is deliberately transparent to the mouse.

  function collectHitRegions(): HitRect[] {
    const out: HitRect[] = [];
    for (const el of document.querySelectorAll<HTMLElement>("[data-hit]")) {
      const r = el.getBoundingClientRect();
      if (r.width > 0 && r.height > 0) {
        out.push({ x: r.x, y: r.y, w: r.width, h: r.height });
      }
    }
    return out;
  }

  let pushQueued = false;
  function pushHitRegions() {
    // After layout, not during it: $effect runs before the browser has
    // recalculated the geometry we are about to measure.
    if (pushQueued) return;
    pushQueued = true;
    requestAnimationFrame(() => {
      pushQueued = false;
      setHitRegions(collectHitRegions()).catch((e) =>
        console.warn("setHitRegions failed", e),
      );
    });
  }

  // Re-measure whenever anything that moves the controls changes. Reading the
  // values here is what subscribes the effect to them.
  $effect(() => {
    void showControls;
    void settingsOpen;
    void subsHidden;
    pushHitRegions();
  });

  onMount(() => {
    // The control bar spans the window width, so a resize moves it even though
    // no Svelte state changed.
    const ro = new ResizeObserver(pushHitRegions);
    ro.observe(document.documentElement);
    return () => ro.disconnect();
  });

  // ── settings window sizing ─────────────────────────────────────────────────

  async function openSettings() {
    settingsOpen = true;
    suppressSave = true;
    try {
      const shortfall = SETTINGS_MIN_HEIGHT - window.innerHeight;
      if (shortfall <= 0) return;

      const w = getCurrentWindow();
      const [outer, pos, scale] = await Promise.all([
        w.outerSize(), w.outerPosition(), w.scaleFactor(),
      ]);
      restoreGeom = { size: outer, pos };

      // Grow by the shortfall rather than setting an absolute height, so the
      // window chrome (however much of it there is) cancels out.
      const grow = Math.round(shortfall * scale);
      // Upward, so the control bar stays under the cursor that just clicked the
      // gear. Clamped at the top edge — off-screen is worse than moved.
      const y = Math.max(0, pos.y - grow);
      await w.setSize(new PhysicalSize(outer.width, outer.height + grow));
      await w.setPosition(new PhysicalPosition(pos.x, y));
    } catch (e) {
      console.warn("openSettings: resize failed", e);
    }
  }

  async function closeSettings() {
    settingsOpen = false;
    const g = restoreGeom;
    restoreGeom = null;
    try {
      if (g) {
        const w = getCurrentWindow();
        await w.setSize(g.size);
        await w.setPosition(g.pos);
      }
    } catch (e) {
      console.warn("closeSettings: restore failed", e);
    } finally {
      // Outlast the move/resize events the restore itself just produced,
      // which would otherwise persist the geometry we are undoing.
      setTimeout(() => (suppressSave = false), 600);
    }
  }
</script>

<main
  class="overlay"
  style="--subtitle-bg-opacity: {opacity};"
  role="application"
>
  {#if settingsOpen}
    <SettingsPanel status={overlay.status} onClose={closeSettings} />
  {/if}

  <!-- subtitles sit ABOVE the control bar so the bar stays at the bottom edge -->
  <div class="stage" class:hidden={subsHidden}>
    <SubtitleView segments={overlay.segments} {fontSize} />
  </div>

  <!-- ControlBar always anchored at the very bottom; shows on hover -->
  <div class="controls" class:visible={showControls} data-hit={showControls ? "" : null}>
    <ControlBar
      status={overlay.status}
      subsHidden={subsHidden}
      onToggleSubs={() => (subsHidden = !subsHidden)}
      onSettingsOpen={openSettings}
    />
  </div>
</main>

<style>
  .overlay {
    height: 100vh;
    width: 100vw;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    box-sizing: border-box;
    padding: 8px;
    background: transparent;
  }

  .controls {
    /* Hidden + non-interactive in click-through mode. */
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.15s ease;
    margin-top: 6px;
  }
  /* Visible & interactive when NOT in click-through mode.
     Always rendered so no mouseenter dependency needed. */
  .controls.visible {
    opacity: 0.5;
    pointer-events: auto;
  }
  .controls.visible:hover {
    opacity: 1;
  }

  .stage {
    display: flex;
    justify-content: center;
    transition: opacity 0.15s ease;
  }
  .stage.hidden {
    opacity: 0;
    pointer-events: none;
  }
</style>
