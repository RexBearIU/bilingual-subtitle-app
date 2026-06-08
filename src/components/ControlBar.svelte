<script lang="ts">
  import * as cmd from "../lib/commands";
  import type { EngineStatus, SourceHint, SubtitleMode, SubtitleUpdate } from "../lib/types";
  import ProcessPicker from "./ProcessPicker.svelte";

  let { status, subsHidden = false, onToggleSubs, onSettingsOpen }: {
    status: EngineStatus | null;
    subsHidden?: boolean;
    onToggleSubs: () => void;
    onSettingsOpen: () => void;
  } = $props();

  let mode         = $derived<SubtitleMode>(status?.mode       ?? "zh");
  let sourceHint   = $derived<SourceHint>(status?.sourceHint   ?? "auto");
  let running      = $derived(status?.capture === "running");
  let clickThrough = $derived(status?.clickThrough ?? false);
  let alwaysOnTop  = $derived(status?.alwaysOnTop  ?? true);
  let musicMode    = $derived(status?.musicMode ?? false);

  async function toggleRun() {
    running ? await cmd.stopCaptioning() : await cmd.startCaptioning();
  }
  async function onMode(e: Event) {
    await cmd.setSubtitleMode((e.target as HTMLSelectElement).value as SubtitleMode);
  }
  async function onSourceHint(e: Event) {
    await cmd.setSourceHint((e.target as HTMLSelectElement).value as SourceHint);
  }

  function dot(s: string | undefined) {
    return s === "ready" || s === "running" ? "ok" : s === "error" ? "err" : "idle";
  }
  // dev injection
  let devSeq = 0;
  const SAMPLES: SubtitleUpdate[] = [
    { id: "", sourceLang: "ko", sourceText: "오늘 진짜 재밌네요", mode: "zh", isFinal: true,
      subtitles: { ko: "오늘 진짜 재밌네요", zh: "今天真的很好玩。" } },
    { id: "", sourceLang: "en", sourceText: "The model runs fully offline.", mode: "zh", isFinal: true,
      subtitles: { en: "The model runs fully offline.", zh: "模型完全離線執行。" } },
  ];
  async function injectSample() {
    devSeq += 1;
    const s = SAMPLES[devSeq % SAMPLES.length];
    await cmd.devInjectSubtitle({ ...s, id: `dev_${devSeq}` });
  }
</script>

<div class="bar">

  <!-- 左側：可縮放的控制群 -->
  <div class="left-group">

    <!-- ① Start / Stop -->
    <button class="run" class:on={running} onclick={toggleRun}>
      {running ? "■ 停止" : "▶ 開始"}
    </button>

    <div class="sep"></div>

    <!-- ② 語言：接收 → 翻譯 -->
    <div class="lang-group" title="接收語言 → 翻譯目標語言">
      <select class="lang-sel" value={sourceHint} onchange={onSourceHint}
              title="Whisper 接收語言（自動 = 每句自動判斷）">
        <option value="auto">自動</option>
        <option value="zh">繁中</option>
        <option value="ko">한국</option>
        <option value="en">EN</option>
      </select>
      <span class="lang-arrow">→</span>
      <select class="lang-sel" value={mode} onchange={onMode}
              title="翻譯目標語言（不翻 = 只顯示原文）">
        <option value="none">不翻</option>
        <option value="zh">繁中</option>
        <option value="ko">한국</option>
        <option value="en">EN</option>
      </select>
    </div>

    <div class="sep"></div>

    <!-- ③ 音訊來源 -->
    <ProcessPicker {status} />

    <!-- ④ 音樂模式 -->
    <button
      class="icon-btn"
      class:active={musicMode}
      onclick={() => cmd.setMusicMode(!musicMode)}
      title={musicMode ? "音樂模式（10s 切片 + beam=3）" : "語音模式（VAD）"}
    >🎵</button>

    <div class="sep"></div>

    <!-- ⑤ 視窗控制 -->
    <button class="icon-btn" class:active={alwaysOnTop}
      onclick={() => cmd.setAlwaysOnTop(!alwaysOnTop)}
      title={alwaysOnTop ? "置頂：開（再按關閉）" : "置頂：關"}>
      📌
    </button>

    <button
      class="txt-btn passthru"
      class:active={clickThrough}
      onclick={() => cmd.setClickThrough(!clickThrough)}
      title={clickThrough ? "穿透：開 — 無法操作覆蓋層，再按解除" : "穿透：關 — 可操作覆蓋層"}
    >{clickThrough ? "⊙ 穿透" : "● 互動"}</button>

    <button
      class="txt-btn"
      class:dim={subsHidden}
      onclick={() => onToggleSubs()}
      title={subsHidden ? "字幕已隱藏（點擊顯示）" : "隱藏字幕"}>
      字幕
    </button>

  </div>

  <!-- spacer -->
  <div class="spacer"></div>

  <!-- 右側：永遠固定在右邊 -->
  <div class="right-group">
    <div class="sep"></div>

    <!-- ⑥ 設定 / Dev -->
    <button class="icon-btn" onclick={() => onSettingsOpen()} title="設定">⚙️</button>
    <button class="dev" onclick={injectSample} title="注入測試字幕 (dev)">✦</button>

    <!-- ⑦ 狀態指示 -->
    <div class="status" title="音訊 · 語音 · 翻譯">
      <span class="dot {dot(status?.capture)}" title="音訊捕捉"></span>
      <span class="dot {dot(status?.asr)}"     title="語音辨識"></span>
      <span class="dot {dot(status?.translation)}" title="翻譯引擎"></span>
    </div>
  </div>

</div>

<style>
  /* ── 整體列 ─────────────────────────────────── */
  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    background: rgba(14, 18, 24, 0.94);
    border-radius: 10px;
    font-size: 12px;
    color: #d7dee6;
    user-select: none;
    width: 100%;
    box-sizing: border-box;
    height: 36px;
  }

  /* ── 分隔線 ─────────────────────────────────── */
  .sep {
    width: 1px;
    height: 16px;
    background: #2e3740;
    flex-shrink: 0;
    margin: 0 2px;
  }

  /* ── 通用按鈕基底 ────────────────────────────── */
  button {
    background: #242b34;
    color: #c8d0da;
    border: 1px solid #343d4a;
    border-radius: 6px;
    cursor: pointer;
    font-size: 12px;
    white-space: nowrap;
    flex-shrink: 0;
    height: 26px;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background 0.08s, border-color 0.08s;
  }
  button:hover { background: #2e3740; border-color: #444f5e; }

  /* 圖示按鈕（正方形） */
  .icon-btn { width: 28px; padding: 0; font-size: 14px; }
  .icon-btn.active { background: #223040; border-color: #2f6fed; }

  /* 文字按鈕（有 padding） */
  .txt-btn { padding: 0 9px; }

  /* ── Start / Stop ────────────────────────────── */
  .run { padding: 0 14px; font-weight: 700; font-size: 12px; letter-spacing: 0.3px; }
  .run.on { background: #6e1e2a; border-color: #903040; color: #ffd0d0; }
  .run:hover { background: #2e3740; }
  .run.on:hover { background: #7e2233; }

  /* ── 語言選擇 ────────────────────────────────── */
  .lang-group {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }
  .lang-sel {
    background: #242b34;
    color: #c8d0da;
    border: 1px solid #343d4a;
    border-radius: 5px;
    height: 26px;
    padding: 0 2px;
    font-size: 12px;
    cursor: pointer;
    outline: none;
    flex-shrink: 0;
    width: 62px;
    text-align-last: center;
    appearance: auto;
  }
  .lang-sel option { text-align: center; }
  .lang-sel:hover  { border-color: #4a5566; }
  .lang-sel:focus  { border-color: #2f6fed; }
  .lang-arrow { font-size: 11px; color: #4a5566; flex-shrink: 0; }

  /* ── 音樂模式 ────────────────────────────────── */
  .icon-btn.active[title*="音樂"] {
    background: #321a58;
    border-color: #6040a0;
    color: #d4a8ff;
  }

  /* ── 置頂 ────────────────────────────────────── */
  .icon-btn.active[title*="置頂"] {
    background: #1a3a28;
    border-color: #2a6040;
  }

  /* ── 穿透 ────────────────────────────────────── */
  .passthru.active { background: #0f2035; border-color: #1e5080; color: #7ab8f0; }

  /* ── 字幕 ────────────────────────────────────── */
  .dim { color: #4e5a68; border-color: #2a3340; }
  .dim:hover { color: #c8d0da; border-color: #343d4a; }

  /* ── 設定 / Dev ──────────────────────────────── */
  .dev { opacity: 0.3; width: 22px; border-color: transparent; background: transparent; font-size: 11px; }
  .dev:hover { opacity: 0.7; background: #242b34; border-color: #343d4a; }

  /* ── 左右群組 ───────────────────────────────── */
  .left-group {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: 1 1 0;
    min-width: 0;
  }
  .right-group {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  /* ── Spacer + 狀態 ───────────────────────────── */
  .spacer { flex: 0 0 8px; }

  .status {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }
  .dot {
    width: 6px; height: 6px; border-radius: 50%;
    display: inline-block; background: #3a4450;
    flex-shrink: 0;
  }
  .dot.ok   { background: #2ec87a; }
  .dot.err  { background: #d95040; }
  .dot.idle { background: #3a4450; }
</style>
