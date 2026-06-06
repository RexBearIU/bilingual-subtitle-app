<script lang="ts">
  import * as cmd from "../lib/commands";
  import type { EngineStatus, SubtitleMode, SubtitleUpdate } from "../lib/types";

  let { status }: { status: EngineStatus | null } = $props();

  let mode = $derived<SubtitleMode>(status?.mode ?? "zh-ko");
  let running = $derived(status?.capture === "running");
  let fontSize = $derived(status?.fontSize ?? 28);
  let clickThrough = $derived(status?.clickThrough ?? false);
  let alwaysOnTop = $derived(status?.alwaysOnTop ?? true);

  async function toggleRun() {
    running ? await cmd.stopCaptioning() : await cmd.startCaptioning();
  }

  async function pickMode(m: SubtitleMode) {
    await cmd.setSubtitleMode(m);
  }

  async function onFont(e: Event) {
    await cmd.setFontSize(Number((e.target as HTMLInputElement).value));
  }

  async function toggleClickThrough(e: Event) {
    await cmd.setClickThrough((e.target as HTMLInputElement).checked);
  }

  // --- dev injection (ADR-0005): exercise the real event path without audio ---
  let devSeq = 0;
  const SAMPLES: Record<SubtitleMode, SubtitleUpdate[]> = {
    "zh-en": [
      { id: "", sourceLang: "ko", sourceText: "오늘 진짜 재밌네요",
        mode: "zh-en", isFinal: true,
        subtitles: { zh: "今天真的很好玩。", en: "This is really fun today." } },
      { id: "", sourceLang: "en", sourceText: "Let's get started with the demo.",
        mode: "zh-en", isFinal: true,
        subtitles: { en: "Let's get started with the demo.", zh: "我們開始示範吧。" } },
    ],
    "zh-ko": [
      { id: "", sourceLang: "zh", sourceText: "這個功能很實用。",
        mode: "zh-ko", isFinal: true,
        subtitles: { zh: "這個功能很實用。", ko: "이 기능 정말 유용해요." } },
      { id: "", sourceLang: "en", sourceText: "The model runs fully offline.",
        mode: "zh-ko", isFinal: true,
        subtitles: { zh: "模型完全離線執行。", ko: "모델이 완전히 오프라인으로 작동합니다." } },
    ],
  };

  async function injectSample() {
    const pool = SAMPLES[mode];
    const sample = pool[devSeq % pool.length];
    devSeq += 1;
    await cmd.devInjectSubtitle({ ...sample, id: `dev_${devSeq}` });
  }

  function statusDot(s: string | undefined) {
    return s === "ready" || s === "running" ? "ok" : s === "error" ? "err" : "idle";
  }
</script>

<div class="bar">
  <button class="run" class:on={running} onclick={toggleRun}>
    {running ? "■ Stop" : "▶ Start"}
  </button>

  <div class="seg">
    <button class:active={mode === "zh-ko"} onclick={() => pickMode("zh-ko")}>zh-ko</button>
    <button class:active={mode === "zh-en"} onclick={() => pickMode("zh-en")}>zh-en</button>
  </div>

  <label class="font">
    A
    <input type="range" min="14" max="64" value={fontSize} oninput={onFont} />
  </label>

  <button
    class="pin"
    class:active={alwaysOnTop}
    onclick={() => cmd.setAlwaysOnTop(!alwaysOnTop)}
    title={alwaysOnTop ? "Click to unpin (allow other windows on top)" : "Pin on top"}
  >
    {alwaysOnTop ? "📌 置頂中" : "📍 未置頂"}
  </button>

  <label class="check">
    <input type="checkbox" checked={clickThrough} onchange={toggleClickThrough} />
    穿透
  </label>

  <button class="dev" onclick={injectSample} title="Emit a test subtitle (dev only)">
    inject
  </button>

  <div class="status">
    <span class="dot {statusDot(status?.capture)}"></span>cap
    <span class="dot {statusDot(status?.asr)}"></span>asr
    <span class="dot {statusDot(status?.translation)}"></span>mt
  </div>
</div>

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background: rgba(18, 22, 28, 0.92);
    border-radius: 10px;
    font-size: 12px;
    color: #d7dee6;
    user-select: none;
  }
  button {
    background: #2a313b;
    color: #d7dee6;
    border: 1px solid #3a434f;
    border-radius: 6px;
    padding: 4px 8px;
    cursor: pointer;
    font-size: 12px;
  }
  button:hover { background: #333c47; }
  .run.on { background: #7a2230; border-color: #99303f; color: #fff; }
  .seg { display: flex; gap: 2px; }
  .seg button.active { background: #2f6fed; border-color: #2f6fed; color: #fff; }
  .pin.active { background: #2a5a3a; border-color: #357a4a; color: #eafff0; }
  .font { display: flex; align-items: center; gap: 6px; }
  .font input { width: 90px; }
  .check { display: flex; align-items: center; gap: 4px; cursor: pointer; }
  .dev { font-style: italic; opacity: 0.85; }
  .status { display: flex; align-items: center; gap: 4px; margin-left: auto; opacity: 0.9; }
  .dot {
    width: 8px; height: 8px; border-radius: 50%;
    display: inline-block; margin-right: 1px; background: #555;
  }
  .dot.ok { background: #3ad07a; }
  .dot.err { background: #e0563a; }
  .dot.idle { background: #5a636e; }
</style>
