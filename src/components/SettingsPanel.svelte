<script lang="ts">
  import * as cmd from "../lib/commands";
  import ProviderList from "./ProviderList.svelte";
  import type { EngineStatus } from "../lib/types";

  let { status, onClose }: { status: EngineStatus | null; onClose: () => void } = $props();

  let fontSize          = $derived(status?.fontSize             ?? 28);
  let opacity           = $derived(status?.subtitleOpacity      ?? 0.55);
  let asrBackend        = $derived(status?.asrBackend           ?? 'whisper');
  let whisperModel      = $derived(status?.whisperModel         ?? 'turbo');
  let sensevoicePrecision = $derived(status?.sensevoicePrecision ?? 'int8');
  let providers   = $derived(status?.translateProviders   ?? []);
  let activeIdx   = $derived(status?.translateActive ?? 0);
  let autoContext = $derived(status?.autoContext ?? '');

  // Fetched once rather than carried on EngineStatus: that is re-broadcast on
  // every RMS update, and this is a few hundred characters that almost never
  // change.


  // Tabs rather than one long column: the panel lives inside the subtitle
  // overlay, which is only a couple hundred px tall. Fitting everything on
  // one page means a settings dialog that swallows the screen.
  type Tab = 'translate' | 'asr' | 'look';
  const TABS: { id: Tab; label: string }[] = [
    { id: 'translate', label: '翻譯' },
    { id: 'asr',       label: '辨識' },
    { id: 'look',      label: '外觀' },
  ];
  let tab = $state<Tab>('translate');

  async function onFont(e: Event) {
    await cmd.setFontSize(Number((e.target as HTMLInputElement).value));
  }
  async function onOpacity(e: Event) {
    await cmd.updateSettings({ subtitleOpacity: Number((e.target as HTMLInputElement).value) });
  }
  async function toggleAsr() {
    // Cycle: Whisper → SenseVoice → Zipformer-KO → Whisper
    const next = asrBackend === 'whisper' ? 'sensevoice'
               : asrBackend === 'sensevoice' ? 'zipformer-ko'
               : 'whisper';
    await cmd.updateSettings({ asrBackend: next });
  }
  async function toggleWhisperModel() {
    await cmd.updateSettings({ whisperModel: whisperModel === 'large' ? 'turbo' : 'large' });
  }
  async function toggleSvPrecision() {
    await cmd.updateSettings({ sensevoicePrecision: sensevoicePrecision === 'fp32' ? 'int8' : 'fp32' });
  }
</script>

<!-- click outside to close. `data-hit` keeps the whole window clickable while
     the panel is open, so that click actually reaches us in auto mode. -->
<div class="backdrop" role="presentation" onclick={onClose} data-hit></div>

<div class="panel" role="dialog">
  <div class="header">
    <span>⚙️ 設定</span>
    <button class="close" onclick={onClose}>✕</button>
  </div>

  <div class="tabs">
    {#each TABS as t (t.id)}
      <button class="tab" class:on={tab === t.id} onclick={() => (tab = t.id)}>{t.label}</button>
    {/each}
  </div>

  <div class="body">
    {#if tab === 'translate'}
    <div class="ctx">
      <div class="ctx-head">
        <span class="ctx-label">這段在講什麼</span>
      </div>
      <p class="ctx-hint">
        {#if autoContext}
          <span class="ctx-auto-tag">自動</span>{autoContext}
        {:else}
          開始播之後會自動聽出這是什麼、有哪些人名，再拿來校準辨識和翻譯。
        {/if}
      </p>
    </div>

    <ProviderList {providers} {activeIdx} />
    {/if}

    {#if tab === 'asr'}
    <!-- ── 辨識 ───────────────────────────────────────────────────── -->
    <div class="row">
      <span class="label">辨識引擎</span>
      <button class="gpu-btn"
              class:sv={asrBackend === 'sensevoice'}
              class:zip={asrBackend === 'zipformer-ko'}
              onclick={toggleAsr}>
        {asrBackend === 'sensevoice' ? 'SenseVoice'
          : asrBackend === 'zipformer-ko' ? 'Zipformer-KO' : 'Whisper'}
      </button>
      <span class="val hint-inline">{asrBackend === 'sensevoice' ? '多語'
        : asrBackend === 'zipformer-ko' ? '韓文快' : '預設'}</span>
    </div>

    {#if asrBackend === 'whisper'}
    <div class="row sub-row">
      <span class="label">模型大小</span>
      <button class="gpu-btn" class:large={whisperModel === 'large'} onclick={toggleWhisperModel}>
        {whisperModel === 'large' ? 'Large-v3 int8' : 'Turbo'}
      </button>
      <span class="val hint-inline">{whisperModel === 'large' ? '高品質' : '較快'}</span>
    </div>
    <p class="hint">Large-v3 int8：品質更好，首次下載需要時間，GPU VRAM ~1.5 GB。</p>
    {/if}

    {#if asrBackend === 'sensevoice'}
    <div class="row sub-row">
      <span class="label">模型精度</span>
      <button class="gpu-btn" class:sv={sensevoicePrecision === 'fp32'} onclick={toggleSvPrecision}>
        {sensevoicePrecision === 'fp32' ? 'float32' : 'int8'}
      </button>
      <span class="val hint-inline">{sensevoicePrecision === 'fp32' ? '更精準' : '較快'}</span>
    </div>
    <p class="hint">float32：完整精度模型 (~220 MB)，準確率更高。</p>
    {/if}

    {#if asrBackend === 'zipformer-ko'}
    <p class="hint">韓文專用 Zipformer：CPU 即時、完整轉錄、口語自然，外來語/夾英較弱。首次自動下載 (~110 MB)。需 sherpa-onnx 環境（同 SenseVoice）。</p>
    {/if}

    <p class="hint">切換引擎或模型後重新 Start 生效。</p>
    {/if}

    {#if tab === 'look'}
    <!-- ── 外觀 ───────────────────────────────────────────────────── -->
    <div class="row">
      <span class="label">A 整體大小</span>
      <input class="slider" type="range" min="14" max="64" value={fontSize} oninput={onFont} />
      <span class="val">{fontSize} px</span>
    </div>
    <p class="hint left">控制列跟著一起縮放。</p>

    <div class="row">
      <span class="label">◐ 字幕透明度</span>
      <input class="slider" type="range" min="0.05" max="1" step="0.05"
             value={opacity} oninput={onOpacity} />
      <span class="val">{Math.round(opacity * 100)} %</span>
    </div>
    <p class="hint left">
      控制列的「互動 / 自動 / 穿透」決定滑鼠行為。自動＝只有控制列和設定接滑鼠，
      字幕區域直接穿透到下層。卡住時按 Ctrl+Alt+P 一定能解除。
    </p>
    {/if}
  </div>
</div>

<style>
  /* ── 背景說明 ─────────────────────────────── */
  .ctx { padding: 4px 14px 8px; }
  .ctx-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 4px;
  }
  .ctx-label { color: #9aa3ae; font-size: 11px; }
  /* Marks the hint as the machine's own words rather than instructions. */
  .ctx-auto-tag {
    display: inline-block;
    margin-right: 5px;
    padding: 0 4px;
    border: 1px solid #39434f;
    border-radius: 3px;
    color: #7a869a;
    font-size: 8px;
    letter-spacing: 0.06em;
    vertical-align: 1px;
  }

  .ctx-hint {
    margin: 4px 0 0;
    font-size: 10px;
    color: #4e5a65;
    line-height: 1.45;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 10;
  }

  .panel {
    position: fixed;
    bottom: 52px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 20;
    width: min(460px, 94vw);
    /* The overlay window is short. It is grown when Settings opens, but that
       can fail (a small screen, a clamped top edge), and a clipped panel with
       no way to reach the rest is worse than a scrollbar. */
    max-height: calc(100vh - 60px);
    overflow-y: auto;
    overscroll-behavior: contain;
    background: rgba(15, 19, 26, 0.97);
    border: 1px solid #333d4a;
    border-radius: 10px;
    backdrop-filter: blur(12px);
    color: #d7dee6;
    font-size: 12px;
    box-shadow: 0 8px 28px rgba(0,0,0,0.75);
    animation: pop 0.1s ease;
  }

  @keyframes pop {
    from { opacity: 0; transform: translateX(-50%) translateY(6px); }
    to   { opacity: 1; transform: translateX(-50%) translateY(0); }
  }

  .header {
    position: sticky;
    top: 0;
    z-index: 1;
    background: rgba(15, 19, 26, 0.97);
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 14px 7px;
    border-bottom: 1px solid #252d38;
    font-weight: 600;
    font-size: 12px;
  }
  .close {
    background: none; border: none; color: #8a93a0;
    cursor: pointer; font-size: 14px; padding: 1px 6px;
    border-radius: 4px; line-height: 1;
  }
  .close:hover { background: #2a313b; color: #d7dee6; }

  .body {
    padding: 0 0 8px;
    /* Every tab is the same height, so switching one does not resize the panel
       under the cursor. Sized to the tallest page; shorter pages leave space at
       the bottom rather than making the window jump.
       Was 288 while a two-row context textarea made the translate tab the
       tallest; that input is gone, so this is back to what the remaining
       pages need. */
    min-height: 268px;
    box-sizing: border-box;
  }

  .tabs {
    display: flex;
    gap: 2px;
    padding: 6px 10px 0;
    border-bottom: 1px solid #252d38;
  }
  .tab {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: #6d7987;
    font-family: inherit;
    font-size: 11px;
    padding: 5px 12px 5px;
    cursor: pointer;
    border-radius: 4px 4px 0 0;
  }
  .tab:hover { color: #b7c2ce; background: #1a222c; }
  .tab.on {
    color: #cfe0f2;
    border-bottom-color: #3a5591;
    font-weight: 600;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 14px;
  }

  .label {
    flex-shrink: 0;
    width: 88px;
    color: #9aa3ae;
    font-size: 11px;
  }

  .slider {
    flex: 1;
    min-width: 0;
  }

  .val {
    flex-shrink: 0;
    width: 46px;
    text-align: right;
    color: #7bcfa0;
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }

  .hint {
    margin: 0 0 4px;
    padding: 0 14px 6px 116px;   /* indent past label width */
    font-size: 10px;
    color: #4e5a65;
    line-height: 1.45;
  }
  /* Full-width variant for text that has no label to line up with. */
  .hint.left { padding-left: 14px; }

  .hint-inline {
    color: #5a636e;
  }

  .gpu-btn {
    background: #2a3d6a; border: 1px solid #3a5591;
    color: #a0c8ff; border-radius: 6px;
    padding: 4px 12px; cursor: pointer; font-size: 12px;
    white-space: nowrap;
    font-family: inherit;
  }
  .gpu-btn:hover { background: #334880; }
  .gpu-btn:disabled {
    opacity: 0.45;
    cursor: default;
    background: #2a3d6a;
  }
  .gpu-btn.sv    { background: #2a4a3a; border-color: #3a7a5a; color: #90e8b0; }
  .gpu-btn.zip   { background: #4a3a2a; border-color: #7a5a3a; color: #e8c090; }
  .gpu-btn.large { background: #3a2a6a; border-color: #5a4aaa; color: #c0a8ff; }

  .sub-row { padding-left: 28px; }

</style>
