<script lang="ts">
  import type { SubtitleUpdate } from "../lib/types";
  import { linesFor } from "../lib/subtitle-lines";
  import { overlay } from "../lib/subtitles.svelte";
  import Icon from "./Icon.svelte";

  let { segments, fontSize }: { segments: SubtitleUpdate[]; fontSize: number } =
    $props();
</script>

<div class="subtitle-stack" style:font-size="{fontSize}px">
  {#if segments.length === 0}
    <div class="placeholder">
      字幕待命中 · waiting for audio
    </div>
  {:else}
    {#each segments as seg (seg.id)}
      {@const lines = linesFor(seg)}
      <div
        class="segment"
        class:partial={!seg.isFinal}
       
      >
        {#each lines as line (line.lang)}
          <div
            class="line"
            class:primary={line.primary}
            class:secondary={!line.primary}
           
          >
            {line.text}
          </div>
        {/each}
        <!-- `data-hit` is what makes this clickable at all: the overlay passes
             the mouse straight through everything untagged (ADR-0012). Only
             this button's own rectangle is registered, so the subtitle it sits
             on keeps letting clicks reach the video underneath. -->
        <button
          class="copy"
          class:copied={overlay.copiedId === seg.id}
          data-hit
          onclick={() => overlay.copySegment(seg.id)}
          onpointerenter={() => overlay.hold(seg.id)}
          onpointerleave={() => overlay.hold(null)}
          aria-label="複製這句"
          title={overlay.copiedId === seg.id ? "已複製" : "複製這句"}
        ><Icon name="copy" size={18} /></button>
      </div>
    {/each}
  {/if}
</div>

<style>
  .subtitle-stack {
    display: flex;
    flex-direction: column;
    gap: 0.45em;
    align-items: center;
    max-width: 100%;
    cursor: default;
  }

  .segment {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 0.15em;
    align-items: center;
    text-align: center;
    /* Right padding leaves the copy button a lane of its own, so a long line
       cannot run underneath it. */
    padding: 0.35em 2.6em 0.35em 0.9em;
    border-radius: 14px;
    /* --subtitle-bg-opacity is set by App.svelte from the saved settings (default 0.55). */
    background: rgba(0, 0, 0, var(--subtitle-bg-opacity, 0.55));
    backdrop-filter: blur(2px);
    max-width: 100%;
    /* Partial segments (source-only, translation pending) are slightly dimmed. */
    transition: opacity 0.15s ease;
  }

  .segment.partial {
    opacity: 0.75;
  }

  .line {
    line-height: 1.25;
    text-shadow: 0 2px 6px rgba(0, 0, 0, 0.9);
    word-break: break-word;
  }

  .primary {
    color: #b8c4d0;
    font-size: 0.82em;
    font-weight: 500;
  }

  .secondary {
    color: #ffffff;
    font-weight: 600;
  }

  /* Sits inside the bubble, dim until pointed at. It cannot be revealed on
     hover-of-the-segment the way a normal web UI would: the segment itself
     never receives the mouse, only this button does. So it is always present
     and always clickable, just quiet. */
  /* Full-height rather than a small square in the corner. It is sized in `em`
     off the subtitle text, so raising the caption font size makes the target
     bigger too — and this is a target you reach for while the line is still on
     screen, so it has to be hittable at a glance. */
  .copy {
    position: absolute;
    top: 0.25em;
    bottom: 0.25em;
    right: 0.3em;
    width: 2em;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    border: none;
    border-radius: 0.35em;
    background: transparent;
    color: #ffffff;
    opacity: 0.22;
    cursor: pointer;
    transition: opacity 0.12s ease, background 0.12s ease;
  }
  .copy:hover {
    opacity: 0.95;
    background: rgba(255, 255, 255, 0.14);
  }
  .copy.copied {
    opacity: 1;
    color: #7fd18a;
  }

  .placeholder {
    padding: 0.35em 0.9em;
    border-radius: 14px;
    background: rgba(0, 0, 0, var(--subtitle-bg-opacity, 0.55));
    backdrop-filter: blur(2px);
    color: #8a93a0;
    font-style: italic;
  }
</style>
