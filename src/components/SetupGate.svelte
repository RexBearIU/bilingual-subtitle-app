<script lang="ts">
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import * as cmd from "../lib/commands";
  import type { SetupProgress, SetupState } from "../lib/commands";

  let { onReady }: { onReady: () => void } = $props();

  let setup = $state<SetupState | null>(null);
  let running = $state(false);
  let failed = $state<string | null>(null);
  // Only the last few lines. uv emits hundreds while resolving, and a panel
  // that grows without bound pushes the overlay off screen.
  let lines = $state<string[]>([]);
  const TAIL = 6;

  let unlisten: UnlistenFn | null = null;

  onMount(() => {
    (async () => {
      setup = await cmd.getSetupState();
      if (setup.ready) return onReady();

      unlisten = await listen<SetupProgress>("setup_progress", (e) => {
        const p = e.payload;
        if (p.done) {
          running = false;
          if (p.ok) onReady();
          else failed = p.message;
          return;
        }
        lines = [...lines, p.line].slice(-TAIL);
      });
    })();
    return () => unlisten?.();
  });

  async function install() {
    failed = null;
    lines = [];
    running = true;
    await cmd.runAsrSetup();
  }
</script>

{#if setup && !setup.ready}
  <!-- `data-hit` on the whole card: this is the one screen where the overlay
       has to take the mouse, because there is nothing behind it worth
       clicking through to yet. -->
  <div class="gate" data-hit>
    <h1>還差一步</h1>
    <p class="lede">
      語音辨識需要一套 Python 環境（約 1.2 GB）。按下去之後全部自動完成，
      不需要開命令列。
    </p>

    {#if running}
      <div class="bar"><div class="fill"></div></div>
      <p class="note">下載中 · 依網速約需數分鐘，可以放著不管</p>
      <pre class="log">{lines.join("\n")}</pre>
    {:else}
      {#if failed}
        <p class="err">{failed}</p>
      {/if}
      {#if setup.canInstall}
        <button class="go" onclick={install}>
          {failed ? "重試" : "開始安裝"}
        </button>
      {:else}
        <p class="err">
          這個版本沒有內含 uv.exe，只能手動安裝 —— 請看 docs/SETUP.md。
        </p>
      {/if}
      <p class="where">安裝位置：{setup.envRoot}</p>
    {/if}
  </div>
{/if}

<style>
  .gate {
    max-width: 30em;
    margin: 0 auto;
    padding: 1.4em 1.6em;
    border-radius: 14px;
    background: rgba(14, 18, 24, 0.97);
    color: #d7dee6;
    font-size: 13px;
    line-height: 1.5;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
  }

  h1 {
    margin: 0 0 0.4em;
    font-size: 17px;
    font-weight: 600;
    color: #fff;
  }

  .lede { margin: 0 0 1.1em; color: #a8b3c0; }

  .go {
    background: #2f6fed;
    color: #fff;
    border: none;
    border-radius: 7px;
    padding: 0.6em 1.4em;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
  }
  .go:hover { background: #4380f5; }

  .where {
    margin: 0.9em 0 0;
    font-size: 11px;
    color: #5a6470;
    word-break: break-all;
  }

  .err {
    margin: 0 0 0.9em;
    padding: 0.6em 0.8em;
    border-radius: 6px;
    background: rgba(110, 30, 42, 0.35);
    border: 1px solid #903040;
    color: #ffd0d0;
    font-size: 12px;
  }

  /* Indeterminate on purpose: uv reports what it is doing but not how far
     through it is, and a bar that invents a percentage is worse than one that
     is honest about only meaning "still working". */
  .bar {
    height: 3px;
    border-radius: 2px;
    background: #242b34;
    overflow: hidden;
  }
  .fill {
    height: 100%;
    width: 35%;
    background: #2f6fed;
    animation: slide 1.4s ease-in-out infinite;
  }
  @keyframes slide {
    0%   { transform: translateX(-100%); }
    100% { transform: translateX(320%); }
  }

  .note { margin: 0.7em 0 0.5em; font-size: 12px; color: #8a93a0; }

  .log {
    margin: 0;
    max-height: 7em;
    overflow: hidden;
    font-size: 10px;
    line-height: 1.45;
    color: #5a6470;
    white-space: pre-wrap;
    word-break: break-all;
  }
</style>
