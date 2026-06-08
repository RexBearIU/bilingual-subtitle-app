<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import ControlBar from "./components/ControlBar.svelte";
  import SettingsPanel from "./components/SettingsPanel.svelte";
  import SubtitleView from "./components/SubtitleView.svelte";
  import { overlay } from "./lib/subtitles.svelte";
  import { getStatus, updateSettings } from "./lib/commands";

  let settingsOpen  = $state(false);
  let subsHidden    = $state(false);   // subtitle visibility toggle

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

  let fontSize     = $derived(overlay.status?.fontSize       ?? 28);
  let clickThrough = $derived(overlay.status?.clickThrough   ?? false);
  let opacity      = $derived(overlay.status?.subtitleOpacity ?? 0.55);

  // Controls are always interactive when not in click-through mode.
  // We no longer rely on mouseenter/leave (unreliable on Tauri transparent windows).
  let showControls = $derived(!clickThrough);
</script>

<main
  class="overlay"
  style="--subtitle-bg-opacity: {opacity};"
  role="application"
>
  {#if settingsOpen}
    <SettingsPanel status={overlay.status} onClose={() => (settingsOpen = false)} />
  {/if}

  <!-- subtitles sit ABOVE the control bar so the bar stays at the bottom edge -->
  <div class="stage" class:hidden={subsHidden}>
    <SubtitleView segments={overlay.segments} {fontSize} />
  </div>

  <!-- ControlBar always anchored at the very bottom; shows on hover -->
  <div class="controls" class:visible={showControls}>
    <ControlBar
      status={overlay.status}
      subsHidden={subsHidden}
      onToggleSubs={() => (subsHidden = !subsHidden)}
      onSettingsOpen={() => (settingsOpen = true)}
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
