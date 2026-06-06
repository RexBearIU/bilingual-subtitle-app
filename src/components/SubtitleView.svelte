<script lang="ts">
  import type { Lang, SubtitleUpdate } from "../lib/types";

  let { update, fontSize }: { update: SubtitleUpdate | null; fontSize: number } =
    $props();

  // Order: source-language line first (the "original"), then the translation(s).
  const LANG_ORDER: Lang[] = ["zh", "ko", "en"];

  let lines = $derived.by(() => {
    if (!update) return [] as { lang: Lang; text: string; primary: boolean }[];
    const out: { lang: Lang; text: string; primary: boolean }[] = [];
    const subs = update.subtitles;
    const src = update.sourceLang;
    if (subs[src]) out.push({ lang: src, text: subs[src]!, primary: true });
    for (const l of LANG_ORDER) {
      if (l !== src && subs[l]) out.push({ lang: l, text: subs[l]!, primary: false });
    }
    return out;
  });
</script>

<div class="subtitle" style:font-size="{fontSize}px" data-tauri-drag-region>
  {#if lines.length === 0}
    <div class="placeholder primary" data-tauri-drag-region>
      字幕待命中 · waiting for audio
    </div>
  {:else}
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
  {/if}
</div>

<style>
  .subtitle {
    display: flex;
    flex-direction: column;
    gap: 0.15em;
    align-items: center;
    justify-content: center;
    text-align: center;
    padding: 0.35em 0.9em;
    border-radius: 14px;
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(2px);
    max-width: 100%;
    cursor: default;
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
    color: #8a93a0;
    font-style: italic;
  }
</style>
