<script lang="ts">
  /**
   * The bar's icon set.
   *
   * Inline SVG rather than emoji: 📌 and ⚙️ render as full-colour glyphs on
   * Windows while ◐ and ⊙ render as monochrome text, so a row mixing them
   * never looks like one set. These inherit `currentColor`, so a button's
   * active and disabled states carry the icon with them.
   *
   * Drawn on a 16-unit grid, stroked rather than filled except where a solid
   * shape reads better at this size (play, stop, the pin's head).
   */
  export type IconName =
    | "play"
    | "stop"
    | "pin"
    | "captions"
    | "gear"
    | "spark"
    | "mouse-off"
    | "mouse-auto"
    | "mouse-on";

  let { name, size = 14 }: { name: IconName; size?: number } = $props();
</script>

<svg
  class="icon"
  width={size}
  height={size}
  viewBox="0 0 16 16"
  fill="none"
  stroke="currentColor"
  stroke-width="1.4"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false"
>
  {#if name === "play"}
    <path d="M5 3.2 12.5 8 5 12.8Z" fill="currentColor" stroke="none" />
  {:else if name === "stop"}
    <rect x="4" y="4" width="8" height="8" rx="1.2" fill="currentColor" stroke="none" />
  {:else if name === "pin"}
    <!-- "Keep on top", drawn literally: an arrow up into a ceiling.
         A pushpin was tried first and read as a dagger at this size — the head
         cannot be made wide enough to tell apart from the shaft at 14 px. -->
    <path d="M3.5 2.6h9" />
    <path d="M8 13.4V5.6" />
    <path d="M4.9 8.7 8 5.4l3.1 3.3" />
  {:else if name === "captions"}
    <rect x="1.8" y="3.4" width="12.4" height="9.2" rx="2" />
    <path d="M6.4 6.9a1.9 1.9 0 1 0 0 2.2M11.4 6.9a1.9 1.9 0 1 0 0 2.2" />
  {:else if name === "gear"}
    <!-- Sliders, not a gear. A gear's teeth collapse into a sun at 14 px;
         three tracks with knobs stay readable and say "settings" just as well. -->
    <path d="M2.5 4.2h11M2.5 8h11M2.5 11.8h11" />
    <circle cx="5.6" cy="4.2" r="1.5" fill="#0e1218" />
    <circle cx="10.4" cy="8" r="1.5" fill="#0e1218" />
    <circle cx="5.6" cy="11.8" r="1.5" fill="#0e1218" />
  {:else if name === "spark"}
    <path d="M8 2.2 9.3 6.7 13.8 8 9.3 9.3 8 13.8 6.7 9.3 2.2 8 6.7 6.7Z" />
  {:else if name === "mouse-off"}
    <!-- Click-through OFF: the window takes every click. A solid disc. -->
    <circle cx="8" cy="8" r="5" fill="currentColor" stroke="none" />
  {:else if name === "mouse-auto"}
    <!-- AUTO: half takes clicks, half passes them through. -->
    <circle cx="8" cy="8" r="5" />
    <path d="M8 3a5 5 0 0 1 0 10Z" fill="currentColor" stroke="none" />
  {:else if name === "mouse-on"}
    <!-- ON: everything passes through. An outline with nothing inside. -->
    <circle cx="8" cy="8" r="5" stroke-dasharray="2.2 2.2" />
  {/if}
</svg>

<style>
  .icon {
    display: block;
    flex-shrink: 0;
  }
</style>
