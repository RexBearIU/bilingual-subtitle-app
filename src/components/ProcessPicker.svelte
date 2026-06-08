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

<svelte:window onkeydown={onKeydown} />

<div class="picker">
  <button
    class="trigger"
    class:active={currentTarget !== null}
    onclick={toggle}
    title={currentTarget ? `捕捉: ${currentTarget.name} (PID ${currentTarget.pid})` : "捕捉系統音訊"}
  >
    🎧 {label}
  </button>

  {#if open}
    <!-- backdrop -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="backdrop" onclick={() => open = false}></div>

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

  .trigger {
    background: #242b34; color: #c8d0da;
    border: 1px solid #343d4a; border-radius: 6px;
    height: 26px;
    padding: 0 9px; cursor: pointer; font-size: 12px;
    white-space: nowrap;
    display: flex; align-items: center;
    transition: background 0.08s, border-color 0.08s;
    max-width: 112px;
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
    min-width: 220px;
    max-height: 260px;
    overflow-y: auto;
    box-shadow: 0 4px 16px rgba(0,0,0,0.5);
  }

  .dropdown-header {
    padding: 7px 12px 5px;
    font-size: 11px;
    color: #5a636e;
    border-bottom: 1px solid #2a313b;
    user-select: none;
  }

  .row {
    display: flex;
    align-items: center;
    width: 100%;
    padding: 7px 12px;
    font-size: 12px;
    color: #d7dee6;
    background: transparent;
    border: none;
    text-align: left;
    cursor: pointer;
    gap: 6px;
  }
  .row:hover { background: #252b34; }
  .row.selected { color: #89c5ff; background: #1a2a3a; }
  .row.muted { color: #4a5566; cursor: default; }
  .row.muted:hover { background: transparent; }
  .row.err { color: #e0563a; cursor: default; font-size: 11px; }
  .row.err:hover { background: transparent; }

  .pid {
    margin-left: auto;
    font-size: 10px;
    color: #4a5566;
    flex-shrink: 0;
  }
</style>
