<script lang="ts">
  import * as cmd from "../lib/commands";
  import type { EngineStatus } from "../lib/types";

  let { status, onClose }: { status: EngineStatus | null; onClose: () => void } = $props();

  let fontSize          = $derived(status?.fontSize             ?? 28);
  let opacity           = $derived(status?.subtitleOpacity      ?? 0.55);
  let llamaGpu          = $derived(status?.llamaGpuLayers       ?? 36);
  let asrBackend        = $derived(status?.asrBackend           ?? 'whisper');
  let whisperModel      = $derived(status?.whisperModel         ?? 'turbo');
  let sensevoicePrecision = $derived(status?.sensevoicePrecision ?? 'int8');

  async function onFont(e: Event) {
    await cmd.setFontSize(Number((e.target as HTMLInputElement).value));
  }
  async function onOpacity(e: Event) {
    await cmd.updateSettings({ subtitleOpacity: Number((e.target as HTMLInputElement).value) });
  }
  async function toggleGpu() {
    await cmd.updateSettings({ llamaGpuLayers: llamaGpu > 0 ? 0 : 36 });
  }
  async function toggleAsr() {
    await cmd.updateSettings({ asrBackend: asrBackend === 'sensevoice' ? 'whisper' : 'sensevoice' });
  }
  async function toggleWhisperModel() {
    await cmd.updateSettings({ whisperModel: whisperModel === 'large' ? 'turbo' : 'large' });
  }
  async function toggleSvPrecision() {
    await cmd.updateSettings({ sensevoicePrecision: sensevoicePrecision === 'fp32' ? 'int8' : 'fp32' });
  }
</script>

<!-- click outside to close -->
<div class="backdrop" role="presentation" onclick={onClose}></div>

<div class="panel" role="dialog">
  <div class="header">
    <span>⚙️ 設定</span>
    <button class="close" onclick={onClose}>✕</button>
  </div>

  <div class="body">
    <!-- row: font size -->
    <div class="row">
      <span class="label">A 字體大小</span>
      <input class="slider" type="range" min="14" max="64" value={fontSize} oninput={onFont} />
      <span class="val">{fontSize} px</span>
    </div>

    <!-- row: opacity -->
    <div class="row">
      <span class="label">◐ 字幕透明度</span>
      <input class="slider" type="range" min="0.05" max="1" step="0.05"
             value={opacity} oninput={onOpacity} />
      <span class="val">{Math.round(opacity * 100)} %</span>
    </div>

    <!-- row: ASR backend -->
    <div class="row">
      <span class="label">辨識引擎</span>
      <button class="gpu-btn" class:sv={asrBackend === 'sensevoice'} onclick={toggleAsr}>
        {asrBackend === 'sensevoice' ? 'SenseVoice' : 'Whisper'}
      </button>
      <span class="val hint-inline">{asrBackend === 'sensevoice' ? '韓文佳' : '預設'}</span>
    </div>

    {#if asrBackend === 'whisper'}
    <!-- row: Whisper model size -->
    <div class="row sub-row">
      <span class="label">模型大小</span>
      <button class="gpu-btn" class:large={whisperModel === 'large'} onclick={toggleWhisperModel}>
        {whisperModel === 'large' ? 'Large-v3 int8' : 'Turbo'}
      </button>
      <span class="val hint-inline">{whisperModel === 'large' ? '高品質' : '較快'}</span>
    </div>
    <p class="hint">Large-v3 int8：品質更好，首次下載需要時間，GPU VRAM ~1.5 GB。切換後重新 Start 生效。</p>
    {/if}

    {#if asrBackend === 'sensevoice'}
    <!-- row: SenseVoice precision -->
    <div class="row sub-row">
      <span class="label">模型精度</span>
      <button class="gpu-btn" class:sv={sensevoicePrecision === 'fp32'} onclick={toggleSvPrecision}>
        {sensevoicePrecision === 'fp32' ? 'float32' : 'int8'}
      </button>
      <span class="val hint-inline">{sensevoicePrecision === 'fp32' ? '更精準' : '較快'}</span>
    </div>
    <p class="hint">float32：完整精度模型 (~220 MB)，準確率更高。切換後重新 Start 生效。</p>
    {/if}

    <p class="hint">切換引擎後重新 Start 生效。</p>

    <!-- row: GPU -->
    <div class="row">
      <span class="label">翻譯引擎</span>
      <button class="gpu-btn" class:cpu={llamaGpu === 0} onclick={toggleGpu}>
        {llamaGpu > 0 ? `GPU（${llamaGpu} layers）` : 'CPU only'}
      </button>
      <span class="val hint-inline">{llamaGpu > 0 ? '~150ms' : '~1.3s'}</span>
    </div>
    <p class="hint">打遊戲時用 CPU 避免 GPU 搶佔。</p>
  </div>
</div>

<style>
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
    padding: 6px 0 4px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 14px;
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

  .hint-inline {
    color: #5a636e;
  }

  .gpu-btn {
    background: #2a3d6a; border: 1px solid #3a5591;
    color: #a0c8ff; border-radius: 6px;
    padding: 4px 12px; cursor: pointer; font-size: 12px;
    white-space: nowrap;
  }
  .gpu-btn:hover { background: #334880; }
  .gpu-btn.cpu { background: #3a2a2a; border-color: #6a3a3a; color: #ffb0a0; }
  .gpu-btn.sv    { background: #2a4a3a; border-color: #3a7a5a; color: #90e8b0; }
  .gpu-btn.large { background: #3a2a6a; border-color: #5a4aaa; color: #c0a8ff; }

  .sub-row { padding-left: 28px; }
</style>
