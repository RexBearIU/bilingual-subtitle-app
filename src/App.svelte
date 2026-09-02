<script lang="ts">
  import { onMount } from "svelte";
  import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import ControlBar from "./components/ControlBar.svelte";
  import SettingsPanel from "./components/SettingsPanel.svelte";
  import SetupGate from "./components/SetupGate.svelte";
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
  // The control bar rides the caption font size rather than having a control
  // of its own. Two sliders for "make it bigger" is one too many, and in
  // practice nobody wants 48px captions under a bar built for 28px ones.
  //
  // Clamped because the two do not want the same range: captions are usable
  // from 14 to 64 px, but a bar at 64/28 would eat the screen and one at 14/28
  // would have unclickable buttons.
  const BAR_BASE_PX = 28; // the caption size the bar's own numbers were drawn for
  // Starts false and is flipped by SetupGate once it has asked the backend.
  // The card itself does not flash on a ready machine — SetupGate draws
  // nothing until it has an answer — and the captions it gates are empty at
  // launch anyway, so the one tick of delay is invisible.
  let setupReady = $state(false);
  let uiScale = $derived(
    Math.min(1.6, Math.max(0.8, (overlay.status?.fontSize ?? BAR_BASE_PX) / BAR_BASE_PX)),
  );

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
  let lastPushed = "";
  function pushHitRegions() {
    // After layout, not during it: $effect runs before the browser has
    // recalculated the geometry we are about to measure.
    if (pushQueued) return;
    pushQueued = true;
    requestAnimationFrame(() => {
      pushQueued = false;
      const regions = collectHitRegions();
      // The observer below fires on every subtitle update. Comparing before
      // sending keeps that from becoming an IPC call per frame; the rectangles
      // only actually move when something opens, closes or resizes.
      const key = JSON.stringify(regions);
      if (key === lastPushed) return;
      lastPushed = key;
      setHitRegions(regions).catch((e) =>
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
    // Each subtitle now carries its own copy button, and the bubble it sits in
    // is re-laid-out on every update — new text, a different width, a moved
    // button. The MutationObserver below cannot see that: rewriting a line
    // changes a text node, which is neither a `data-hit` element nor an
    // attribute, so the rectangle would keep pointing at where the button used
    // to be and the click would fall through to the video.
    void overlay.segments;
    pushHitRegions();
  });

  onMount(() => {
    // The control bar spans the window width, so a resize moves it even though
    // no Svelte state changed.
    const ro = new ResizeObserver(pushHitRegions);
    ro.observe(document.documentElement);

    // A `data-hit` element can appear without App knowing: the audio-source
    // list is a child component's own state. Watching the DOM rather than
    // adding another prop means the next popup cannot forget to say so —
    // which is how the source list ended up unclickable in `auto` mode.
    const mo = new MutationObserver((records) => {
      const touchesHitRegion = records.some((r) => {
        if (r.type === "attributes") return true;
        const nodes = [...r.addedNodes, ...r.removedNodes];
        return nodes.some(
          (n) =>
            n instanceof HTMLElement &&
            (n.hasAttribute("data-hit") || n.querySelector("[data-hit]") !== null),
        );
      });
      if (touchesHitRegion) pushHitRegions();
    });
    mo.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["data-hit"],
    });

    return () => {
      ro.disconnect();
      mo.disconnect();
    };
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
  style="--subtitle-bg-opacity: {opacity}; --ui-scale: {uiScale};"
  role="application"
>
  <!-- Always mounted, never conditional: it is the thing that *asks* whether an
       environment exists, so gating it on the answer means it never asks. It
       renders nothing until it knows, and nothing at all when the answer is
       yes. (It was conditional once. The card never appeared.)

       Why it exists: with no Python environment the sidecar falls back to
       `python` on PATH, loads a model, and then fails every inference with a
       500. Offering Start in that state is offering a button that breaks. -->
  <SetupGate onReady={() => (setupReady = true)} />

  {#if settingsOpen}
    <SettingsPanel status={overlay.status} onClose={closeSettings} />
  {/if}

  <!-- Deliberately NOT `data-hit`: a toast that swallowed clicks for a second
       and a half after every copy would be worse than no toast. -->
  {#if overlay.copiedNote}
    <div class="toast">{overlay.copiedNote}</div>
  {/if}

  <!-- subtitles sit ABOVE the control bar so the bar stays at the bottom edge -->
  <div class="stage" class:hidden={subsHidden}>
    {#if setupReady}
      <SubtitleView segments={overlay.segments} {fontSize} />
    {/if}
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

  .toast {
    align-self: center;
    margin-bottom: 6px;
    padding: 4px 12px;
    border-radius: 999px;
    background: rgba(20, 44, 26, 0.92);
    border: 1px solid #2f6b3c;
    color: #b8f0c2;
    font-size: 12px;
    font-weight: 600;
    pointer-events: none;
    animation: toast-in 0.14s ease;
  }
  @keyframes toast-in {
    from { opacity: 0; transform: translateY(4px); }
    to   { opacity: 1; transform: none; }
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
