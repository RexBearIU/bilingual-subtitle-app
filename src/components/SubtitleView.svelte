<script lang="ts">
  import type { Lang, SubtitleUpdate } from "../lib/types";

  let { segments, fontSize }: { segments: SubtitleUpdate[]; fontSize: number } =
    $props();

  // Order: source-language line first (original), then the translation(s).
  const LANG_ORDER: Lang[] = ["zh", "ko", "en"];

  function linesFor(update: SubtitleUpdate) {
    const out: { lang: Lang; text: string; primary: boolean }[] = [];
    const subs = update.subtitles;
    const src = update.sourceLang as Lang;
    if (subs[src]) out.push({ lang: src, text: subs[src]!, primary: true });
    for (const l of LANG_ORDER) {
      if (l !== src && subs[l]) out.push({ lang: l, text: subs[l]!, primary: false });
    }
    return out;
  }
</script>

<div class="subtitle-stack" style:font-size="{fontSize}px" data-tauri-drag-region>
  {#if segments.length === 0}
    <div class="placeholder" data-tauri-drag-region>
      字幕待命中 · waiting for audio
    </div>
  {:else}
    {#each segments as seg (seg.id)}
      {@const lines = linesFor(seg)}
      <div
        class="segment"
        class:partial={!seg.isFinal}
        data-tauri-drag-region
      >
        {#each lines as line (line.lang)}
          <div
            class="line"
            class:primary={line.primary}
            class:secondary={!line.primary}
            data-tauri-drag-region
          >
            {line.text}
          </div>
        {/each}
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
    display: flex;
    flex-direction: column;
    gap: 0.15em;
    align-items: center;
    text-align: center;
    padding: 0.35em 0.9em;
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
    color: #ffffff;
    font-weight: 600;
  }

  .secondary {
    color: #b8c4d0;
    font-size: 0.82em;
    font-weight: 500;
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
