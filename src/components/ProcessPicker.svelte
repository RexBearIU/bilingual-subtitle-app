<script lang="ts">
  import { listAudioProcesses, setCaptureProcess } from "../lib/commands";
  import type { AudioProcess, EngineStatus } from "../lib/types";

  let { status }: { status: EngineStatus | null } = $props();

  let open = $state(false);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let processes = $state<AudioProcess[]>([]);

  let currentTarget = $derived(status?.captureTarget ?? null);
  let label = $derived(currentTarget ? currentTarget.name : "系統");

  async function toggle() {
    if (open) {
      open = false;
      return;
    }
    loading = true;
    error = null;
    processes = [];
    try {
      processes = await listAudioProcesses();
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
    open = true;
  }

  async function select(p: AudioProcess | null) {
    open = false;
    try {
      if (p) {
        await setCaptureProcess(p.pid, p.name);
      } else {
        await setCaptureProcess(0, "");
      }
    } catch (e) {
      console.error("setCaptureProcess failed:", e);
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") open = false;
  }
</script>

<!-- Also on blur: while the list is open the whole window takes the mouse, so
     clicking away onto whatever is playing behind it neither reaches the
     backdrop nor closes the list — and the overlay goes on blocking the
     cursor. Losing focus is that gesture, and it is the one people reach for
     when they opened the list and changed their mind. -->
<svelte:window onkeydown={onKeydown} onblur={() => (open = false)} />

<div class="picker">
  <button
    class="trigger"
    class:active={currentTarget !== null}
    onclick={toggle}
    title={currentTarget ? `捕捉: ${currentTarget.name} (PID ${currentTarget.pid})` : "捕捉系統音訊"}
  >
    🎧 {label}
  </button>

  {#if status?.message}
    <div class="loopback-err">⚠ {status.message}</div>
  {/if}

  {#if open}
    <!-- Backdrop, and the window's hit region while the list is open. The
         list is positioned above the bar, outside the control bar's own
         rectangle, so in `auto` click-through the mouse passed straight
         through it and the entries could not be clicked. `data-hit` on a
         full-window backdrop is the same fix the settings panel uses. -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="backdrop" data-hit onclick={() => open = false}></div>

    <div class="dropdown">
      <div class="dropdown-header">選擇捕捉來源</div>

      {#if loading}
        <div class="row muted">載入中…</div>
      {:else if error}
        <div class="row err">錯誤: {error}</div>
      {:else}
        <!-- System-wide option -->
        <button
          class="row"
          class:selected={currentTarget === null}
          onclick={() => select(null)}
        >
          🖥 系統（全域）
        </button>

        {#if processes.length === 0}
          <div class="row muted">目前沒有應用程式有音訊輸出</div>
        {:else}
          {#each processes as p (p.pid)}
            <button
              class="row"
              class:selected={currentTarget?.pid === p.pid}
              onclick={() => select(p)}
            >
              {p.name}
              <span class="pid">PID {p.pid}</span>
            </button>
          {/each}
        {/if}
      {/if}
    </div>
  {/if}
</div>

<style>
  .picker {
    position: relative;
    flex-shrink: 0;
  }

  /* Sizes come from the control bar's tokens (--btn / --fs / --ui-scale),
     which this inherits as an ordinary custom property through the DOM.
     They were hardcoded at 26px/12px, so raising the caption size grew every
     other control and left this one behind, visibly the odd one out. */
  .trigger {
    background: #242b34; color: #c8d0da;
    border: 1px solid #343d4a; border-radius: 5px;
    height: var(--btn, 24px);
    padding: 0 calc(7px * var(--ui-scale, 1));
    cursor: pointer;
    font-size: var(--fs, 11px);
    white-space: nowrap;
    display: flex; align-items: center;
    transition: background 0.08s, border-color 0.08s;
    max-width: calc(112px * var(--ui-scale, 1));
    overflow: hidden;
    text-overflow: ellipsis;
    flex-shrink: 0;
  }
  .trigger:hover { background: #2e3740; border-color: #444f5e; }
  .trigger.active {
    background: #162540;
    border-color: #2f6fed;
    color: #89c5ff;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 99;
  }

  .dropdown {
    position: absolute;
    bottom: calc(100% + 5px);
    left: 0;
    z-index: 100;
    background: #1a1f27;
    border: 1px solid #3a434f;
    border-radius: 8px;
    min-width: calc(220px * var(--ui-scale, 1));
    max-height: calc(260px * var(--ui-scale, 1));
    overflow-y: auto;
    box-shadow: 0 4px 16px rgba(0,0,0,0.5);
  }

  .dropdown-header {
    padding: calc(7px * var(--ui-scale, 1)) calc(12px * var(--ui-scale, 1)) calc(5px * var(--ui-scale, 1));
    font-size: calc(10px * var(--ui-scale, 1));
    color: #5a636e;
    border-bottom: 1px solid #2a313b;
    user-select: none;
  }

  .row {
    display: flex;
    align-items: center;
    width: 100%;
    padding: calc(7px * var(--ui-scale, 1)) calc(12px * var(--ui-scale, 1));
    font-size: var(--fs, 11px);
    color: #d7dee6;
    background: transparent;
    border: none;
    text-align: left;
    cursor: pointer;
    gap: calc(6px * var(--ui-scale, 1));
  }
  .row:hover { background: #252b34; }
  .row.selected { color: #89c5ff; background: #1a2a3a; }
  .row.muted { color: #4a5566; cursor: default; }
  .row.muted:hover { background: transparent; }
  .row.err { color: #e0563a; cursor: default; font-size: calc(10px * var(--ui-scale, 1)); }
  .row.err:hover { background: transparent; }

  .pid {
    margin-left: auto;
    font-size: calc(10px * var(--ui-scale, 1));
    color: #4a5566;
    flex-shrink: 0;
  }

  .loopback-err {
    position: absolute;
    bottom: calc(100% + 4px);
    left: 0;
    z-index: 101;
    background: rgba(30, 10, 10, 0.97);
    border: 1px solid #6a2a2a;
    border-radius: 6px;
    padding: calc(4px * var(--ui-scale, 1)) calc(8px * var(--ui-scale, 1));
    font-size: calc(10px * var(--ui-scale, 1));
    color: #e08070;
    white-space: pre-wrap;
    word-break: break-all;
    max-width: 380px;
    line-height: 1.4;
  }
</style>
