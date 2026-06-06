<script lang="ts">
  import { onMount } from "svelte";
  import ControlBar from "./components/ControlBar.svelte";
  import SubtitleView from "./components/SubtitleView.svelte";
  import { overlay } from "./lib/subtitles.svelte";
  import { getStatus } from "./lib/commands";

  // Show the control bar on hover; the subtitle stays visible always.
  let hovering = $state(false);

  onMount(() => {
    let disconnected = false;
    (async () => {
      await overlay.connect();
      // Hydrate initial status (mode / font size / click-through).
      try {
        overlay.status = await getStatus();
      } catch (e) {
        console.error("getStatus failed", e);
      }
      if (disconnected) overlay.disconnect();
    })();
    return () => {
      disconnected = true;
      overlay.disconnect();
    };
  });

  let fontSize = $derived(overlay.status?.fontSize ?? 28);
  let clickThrough = $derived(overlay.status?.clickThrough ?? false);
  // When click-through is on the window passes the mouse through, so the control
  // bar is unreachable anyway — hide it for a clean caption-only overlay.
  let showControls = $derived(hovering && !clickThrough);

  $effect(() => {
    // No mouseleave fires once the mouse starts passing through, so reset here.
    if (clickThrough) hovering = false;
  });
</script>

<main
  class="overlay"
  onmouseenter={() => (hovering = true)}
  onmouseleave={() => (hovering = false)}
  role="application"
>
  <div class="controls" class:visible={showControls}>
    <ControlBar status={overlay.status} />
  </div>

  <div class="stage">
    <SubtitleView update={overlay.current} {fontSize} />
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
    opacity: 0;
    transform: translateY(-4px);
    transition: opacity 0.12s ease, transform 0.12s ease;
    pointer-events: none;
    margin-bottom: 8px;
  }
  .controls.visible {
    opacity: 1;
    transform: none;
    pointer-events: auto;
  }
  .stage {
    display: flex;
    justify-content: center;
  }
</style>
