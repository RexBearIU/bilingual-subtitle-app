<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import * as cmd from "../lib/commands";
  import type { ClickThroughMode, EngineStatus, SourceHint, SubtitleMode, SubtitleUpdate } from "../lib/types";
  import ProcessPicker from "./ProcessPicker.svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  let { status, subsHidden = false, onToggleSubs, onSettingsOpen }: {
    status: EngineStatus | null;
    subsHidden?: boolean;
    onToggleSubs: () => void;
    onSettingsOpen: () => void;
  } = $props();

  let mode         = $derived<SubtitleMode>(status?.mode       ?? "zh");
  let sourceHint   = $derived<SourceHint>(status?.sourceHint   ?? "auto");
  let running      = $derived(status?.capture === "running");
  let clickThrough = $derived<ClickThroughMode>(status?.clickThrough ?? "auto");
  let alwaysOnTop  = $derived(status?.alwaysOnTop  ?? true);

  async function toggleRun() {
    running ? await cmd.stopCaptioning() : await cmd.startCaptioning();
  }
  async function onMode(e: Event) {
    await cmd.setSubtitleMode((e.target as HTMLSelectElement).value as SubtitleMode);
  }
  async function onSourceHint(e: Event) {
    await cmd.setSourceHint((e.target as HTMLSelectElement).value as SourceHint);
  }

  // off → auto → on → off. `auto` is the resting state: the controls stay
  // clickable while the empty parts of the overlay stop swallowing clicks meant
  // for whatever is playing behind it.
  const CT_ORDER: ClickThroughMode[] = ["off", "auto", "on"];
  const CT_ICON: Record<ClickThroughMode, IconName> = {
    off:  "mouse-off",
    auto: "mouse-auto",
    on:   "mouse-on",
  };
  const CT_TITLE: Record<ClickThroughMode, string> = {
    off:  "互動：整個視窗都接滑鼠，空白處也會擋住下層。點一下切到「自動」",
    auto: "自動：只有控制列跟設定接滑鼠，字幕區域直接穿透。點一下切到「穿透」",
    on:   "穿透：整個視窗都不接滑鼠（托盤或 Ctrl+Alt+P 可解除）。點一下切到「互動」",
  };
  // Once captioning is running the bar has done its job, so it folds down to
  // the one control still worth reaching — stop — plus the status lights.
  // Hovering brings the rest back, which keeps every setting reachable without
  // leaving a full row of buttons sitting over whatever is playing behind it.
  let hovered = $state(false);
  let collapsed = $derived(running && !hovered);

  function cycleClickThrough() {
    const next = CT_ORDER[(CT_ORDER.indexOf(clickThrough) + 1) % CT_ORDER.length];
    return cmd.setClickThrough(next);
  }

  // `data-tauri-drag-region` does not fire in this window, so the drag is
  // started explicitly. Doing it by hand is better anyway: it is greppable,
  // and it can refuse the drag when the press did not start on the handle.
  /** Movement, in px, that turns a press into a drag rather than a click. */
  const DRAG_THRESHOLD = 5;

  async function beginWindowDrag() {
    try {
      await getCurrentWindow().startDragging();
    } catch (err) {
      console.warn("startDragging failed", err);
    }
  }

  /**
   * Let a press on a button become a drag, without costing it its click.
   *
   * `startDragging` hands the mouse to the OS move loop, which never returns
   * the `pointerup` — so a button that starts a drag on `pointerdown` can
   * never fire, which is exactly how these buttons broke once before. Waiting
   * for real movement first separates the two intents: a press that travels is
   * a drag, one that does not is a click, and the click is left alone because
   * nothing here calls `preventDefault` until the drag actually starts.
   */
  function armDragThreshold(e: PointerEvent) {
    const x0 = e.clientX;
    const y0 = e.clientY;
    const stop = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
    };
    const onMove = (m: PointerEvent) => {
      if (Math.hypot(m.clientX - x0, m.clientY - y0) < DRAG_THRESHOLD) return;
      stop();
      void beginWindowDrag();
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
  }

  async function startDrag(e: PointerEvent) {
    if (e.button !== 0) return;
    const el = e.target as HTMLElement;

    // A `select` opens its menu on pointerdown, so its press is never ours.
    if (el.closest("select")) return;

    // A button keeps its click unless the press turns out to be a drag.
    if (el.closest("button")) {
      armDragThreshold(e);
      return;
    }

    // Bare bar: nothing else wants this press, so take it immediately.
    e.preventDefault();
    await beginWindowDrag();
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

<!-- The bar is the window's drag handle: in `auto` mode the subtitle area no
     longer takes the mouse, so it can no longer be dragged. `onpointerdown` on
     every container with bare surface, because the handler must see the press
     land on the container itself — a press on a child button is that button's. -->
<div
  class="bar"
  class:collapsed
  role="toolbar"
  tabindex="-1"
  aria-label="字幕控制列"
  onpointerdown={startDrag}
  onmouseenter={() => (hovered = true)}
  onmouseleave={() => (hovered = false)}
>

  <!-- 左側：可縮放的控制群 -->
  <div class="left-group" role="presentation" onpointerdown={startDrag}>

    <!-- ① Start / Stop -->
    <button
      class="run"
      class:on={running}
      onclick={toggleRun}
      title={running ? "停止" : "開始"}
      aria-label={running ? "停止" : "開始"}
    >
      <Icon name={running ? "stop" : "play"} />
    </button>

    {#if !collapsed}
    <div class="sep"></div>

    <!-- ② 語言：接收 → 翻譯 -->
    <div class="lang-group" title="接收語言 → 翻譯目標語言">
      <select class="lang-sel" value={sourceHint} onchange={onSourceHint}
              title="Whisper 接收語言（自動 = 每句自動判斷）">
        <option value="auto">自動</option>
        <!-- Not 繁/簡: whisper takes a language, not a script, so offering the
             choice here would promise something it cannot do. -->
        <option value="zh">中文</option>
        <option value="ko">한국</option>
        <option value="en">EN</option>
      </select>
      <span class="lang-arrow">→</span>
      <select class="lang-sel" value={mode} onchange={onMode}
              title="翻譯目標語言（不翻 = 只顯示原文）">
        <option value="none">不翻</option>
        <option value="zh">繁中</option>
        <option value="zh-hans">简中</option>
        <option value="ko">한국</option>
        <option value="en">EN</option>
      </select>
    </div>

    <div class="sep"></div>

    <!-- ③ 音訊來源 -->
    <ProcessPicker {status} />

    <div class="sep"></div>

    <!-- ④ 視窗控制 -->
    <button class="icon-btn" class:active={alwaysOnTop}
      onclick={() => cmd.setAlwaysOnTop(!alwaysOnTop)}
      aria-label="視窗置頂"
      title={alwaysOnTop ? "置頂：開（再按關閉）" : "置頂：關"}>
      <Icon name="pin" />
    </button>

    <button
      class="icon-btn passthru ct-{clickThrough}"
      onclick={cycleClickThrough}
      aria-label="滑鼠穿透模式"
      title={CT_TITLE[clickThrough]}
    ><Icon name={CT_ICON[clickThrough]} /></button>

    <button
      class="icon-btn"
      class:dim={subsHidden}
      onclick={() => onToggleSubs()}
      aria-label="顯示或隱藏字幕"
      title={subsHidden ? "字幕已隱藏（點擊顯示）" : "隱藏字幕"}>
      <Icon name="captions" />
    </button>
    {/if}

  </div>

  <div class="spacer"></div>

  <!-- 右側：永遠固定在右邊 -->
  <div class="right-group" role="presentation" onpointerdown={startDrag}>
    {#if !collapsed}
    <div class="sep"></div>

    <!-- ⑤ 設定 / Dev -->
    <button class="icon-btn" onclick={() => onSettingsOpen()} aria-label="設定" title="設定">
      <Icon name="gear" />
    </button>
    <button class="dev" onclick={injectSample} aria-label="注入測試字幕" title="注入測試字幕 (dev)">
      <Icon name="spark" size={12} />
    </button>
    {/if}

    <!-- ⑥ 狀態指示 -->
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
    /* One place to resize the whole bar. Everything below is expressed in
       these, so the row cannot end up with a 24px button in a 36px bar — which
       is what happened every previous time a size was nudged by hand. */
    /* Every size below is one of these times --ui-scale (a saved setting,
       0.7-1.8), so the whole row grows and shrinks together instead of the
       buttons outrunning the text. */
    --btn: calc(24px * var(--ui-scale, 1));   /* button edge; the row's tallest element */
    --fs: calc(11px * var(--ui-scale, 1));    /* label and control text */
    --pad-x: calc(6px * var(--ui-scale, 1));  /* bar's own side padding */
    --gap: calc(3px * var(--ui-scale, 1));

    display: flex;
    align-items: center;
    gap: var(--gap);
    /* Every part of the bar drags, buttons included — they keep `cursor:
       pointer` because a press there is a click until it starts moving. */
    cursor: grab;
    padding: calc(3px * var(--ui-scale, 1)) var(--pad-x);
    background: rgba(14, 18, 24, 0.94);
    border-radius: 9px;
    font-size: var(--fs);
    color: #d7dee6;
    user-select: none;
    /* Hug the controls. The bar used to span the window with a flexible
       `.spacer` shoving the two groups to opposite edges, which on a wide
       overlay is mostly empty bar sitting on top of the video. Nothing needed
       that slack once the whole bar became draggable (ADR-0016) - the spacer
       had been the grab handle. */
    width: fit-content;
    max-width: 100%;
    margin: 0 auto;
    box-sizing: border-box;
    transition: width 0.12s ease;
    /* Derived, not chosen: the button plus its padding, so the bar is exactly
       as tall as it needs to be. */
    height: calc(var(--btn) + 5px);
  }

  /* ── 分隔線 ─────────────────────────────────── */
  .sep {
    width: 1px;
    height: calc(var(--btn) * 0.58);
    background: #2e3740;
    flex-shrink: 0;
    margin: 0 2px;
  }

  /* ── 通用按鈕基底 ────────────────────────────── */
  button {
    background: #242b34;
    color: #c8d0da;
    border: 1px solid #343d4a;
    border-radius: 5px;
    cursor: pointer;
    font-size: var(--fs);
    white-space: nowrap;
    flex-shrink: 0;
    height: var(--btn);
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background 0.08s, border-color 0.08s;
  }
  button:hover { background: #2e3740; border-color: #444f5e; }

  /* 圖示按鈕（正方形） */
  .icon-btn {
    width: var(--btn);
    padding: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .icon-btn.active { background: #223040; border-color: #2f6fed; }

  /* ── Start / Stop ────────────────────────────── */
  /* Icon only. The play/stop pair is the most universally read symbol on the
     bar, and the word beside it was the widest thing in the row — it also had
     to be conditionally hidden when the bar collapsed, which is a whole
     behaviour that stops existing once the label does. The tooltip and
     aria-label still say 開始 / 停止. */
  .run {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--btn);
    padding: 0;
  }
  .run.on { background: #6e1e2a; border-color: #903040; color: #ffd0d0; }
  .run:hover { background: #2e3740; }
  .run.on:hover { background: #7e2233; }

  /* ── 語言選擇 ────────────────────────────────── */
  .lang-group {
    display: flex;
    align-items: center;
    gap: var(--gap);
    flex-shrink: 0;
  }
  .lang-sel {
    background: #242b34;
    color: #c8d0da;
    border: 1px solid #343d4a;
    border-radius: 5px;
    height: var(--btn);
    padding: 0 2px;
    font-size: var(--fs);
    cursor: pointer;
    outline: none;
    flex-shrink: 0;
    width: calc(62px * var(--ui-scale, 1));
    text-align-last: center;
    appearance: auto;
  }
  .lang-sel option { text-align: center; }
  .lang-sel:hover  { border-color: #4a5566; }
  .lang-sel:focus  { border-color: #2f6fed; }
  .lang-arrow { font-size: var(--fs); color: #4a5566; flex-shrink: 0; }

  /* ── 置頂 ────────────────────────────────────── */
  .icon-btn.active[aria-label="視窗置頂"] {
    background: #1a3a28;
    border-color: #2a6040;
  }

  /* ── 穿透 ────────────────────────────────────── */
  .passthru.ct-auto { background: #14263a; border-color: #2a5f88; color: #86c5ea; }
  .passthru.ct-on   { background: #0f2035; border-color: #1e5080; color: #7ab8f0; }

  /* ── 字幕 ────────────────────────────────────── */
  .dim { color: #4e5a68; border-color: #2a3340; }
  .dim:hover { color: #c8d0da; border-color: #343d4a; }

  /* ── 設定 / Dev ──────────────────────────────── */
  .dev {
    opacity: 0.3;
    width: calc(var(--btn) * 0.9);
    padding: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-color: transparent;
    background: transparent;
  }
  .dev:hover { opacity: 0.7; background: #242b34; border-color: #343d4a; }

  /* ── 左右群組 ───────────────────────────────── */
  .bar:active { cursor: grabbing; }

  /* Folded down while captioning runs.
     `fit-content`, not `auto`: the bar is a block-level flex container, so
     `auto` still resolves to the full width of the row and the spacer shoves
     the stop button and the status lights to opposite edges. */
  .bar.collapsed {
    gap: calc(var(--gap) * 2);
  }
  .bar.collapsed .sep {
    display: none;
  }

  .left-group {
    display: flex;
    align-items: center;
    gap: var(--gap);
    /* Shrinks when the bar is narrow, but does NOT grow — the slack belongs to
       `.spacer`, which keeps the settings group pinned to the right edge. */
    flex: 0 1 auto;
    min-width: 0;
  }
  .right-group {
    display: flex;
    align-items: center;
    gap: var(--gap);
    flex-shrink: 0;
  }

  /* ── Spacer + 狀態 ───────────────────────────── */
  /* Holds the two groups apart. Nothing is drawn in it: the whole bar drags
     now, so it does not have to advertise itself as the one place that does. */
  /* Was `flex: 1` - the empty middle of a full-width bar. Kept as a fixed
     breath between the two groups now that the bar hugs its content. */
  .spacer {
    width: calc(var(--gap) * 2);
    flex: 0 0 auto;
  }

  .status {
    display: flex;
    align-items: center;
    gap: var(--gap);
    flex-shrink: 0;
  }
  .dot {
    width: calc(6px * var(--ui-scale, 1));
    height: calc(6px * var(--ui-scale, 1));
    border-radius: 50%;
    display: inline-block; background: #3a4450;
    flex-shrink: 0;
  }
  .dot.ok   { background: #2ec87a; }
  .dot.err  { background: #d95040; }
  .dot.idle { background: #3a4450; }
</style>
