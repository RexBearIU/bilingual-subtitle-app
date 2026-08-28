<script lang="ts">
  import * as cmd from "../lib/commands";
  import type { ProviderDraft, ProviderInfo } from "../lib/types";

  let { providers, activeIdx }: { providers: ProviderInfo[]; activeIdx: number } = $props();

  // The list the panel edits. Rebuilt from `providers` whenever the backend
  // publishes a new one, except while a form is open — otherwise a status
  // broadcast (they arrive on every RMS update) would wipe what is being typed.
  let draft = $state<ProviderDraft[]>([]);
  let editing = $state<number | null>(null);
  let adding = $state(false);
  let error = $state("");
  let busy = $state(false);

  /** Key input for the row being edited or added; never pre-filled. */
  let keyDraft = $state("");
  let form = $state({ name: "", baseUrl: "", model: "" });

  let presets = $state<string[]>([]);
  cmd.translatePresetNames().then((n) => (presets = n)).catch(() => {});

  $effect(() => {
    // Read `providers` so this re-runs when the backend list changes.
    const incoming = providers;
    if (editing !== null || adding) return;
    draft = incoming.map((p) => ({ name: p.name, baseUrl: p.baseUrl, model: p.model }));
  });

  async function commit(next: ProviderDraft[]) {
    busy = true;
    error = "";
    try {
      await cmd.setTranslateProviders(next);
      editing = null;
      adding = false;
      keyDraft = "";
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  function openAdd() {
    form = { name: "", baseUrl: "", model: "" };
    keyDraft = "";
    error = "";
    adding = true;
    editing = null;
  }

  function openEdit(i: number) {
    const p = providers[i];
    form = { name: p.name, baseUrl: p.baseUrl, model: p.model };
    keyDraft = "";
    error = "";
    editing = i;
    adding = false;
  }

  function cancel() {
    adding = false;
    editing = null;
    keyDraft = "";
    error = "";
    draft = providers.map((p) => ({ name: p.name, baseUrl: p.baseUrl, model: p.model }));
  }

  /** A preset name fills its own URL and model in, so only a key is needed. */
  function onNameInput(v: string) {
    form.name = v;
  }

  async function submit() {
    const name = form.name.trim();
    if (!name) {
      error = "請填名稱";
      return;
    }
    const entry: ProviderDraft = {
      name,
      baseUrl: form.baseUrl.trim(),
      model: form.model.trim(),
    };
    // Only send the key when one was typed: absent means "keep what is stored",
    // which is what an edit that does not touch the key should do.
    if (keyDraft.trim()) entry.apiKey = keyDraft.trim();

    const next = providers.map((p) => ({ name: p.name, baseUrl: p.baseUrl, model: p.model }));
    if (editing !== null) {
      // A rename loses the stored key, since keys are carried forward by name.
      if (entry.name !== providers[editing].name && !entry.apiKey) {
        error = "改名後要重新輸入金鑰";
        return;
      }
      next[editing] = entry;
    } else {
      if (next.some((p) => p.name === entry.name)) {
        error = `已經有一個叫 ${entry.name} 的供應商`;
        return;
      }
      if (!entry.apiKey) {
        error = "請填 API 金鑰";
        return;
      }
      next.push(entry);
    }
    await commit(next);
  }

  async function remove(i: number) {
    const next = providers
      .filter((_, j) => j !== i)
      .map((p) => ({ name: p.name, baseUrl: p.baseUrl, model: p.model }));
    await commit(next);
  }

  async function clearKey(i: number) {
    const next = providers.map((p, j) => ({
      name: p.name,
      baseUrl: p.baseUrl,
      model: p.model,
      // "" clears it; the environment then takes over again if it has one.
      ...(j === i ? { apiKey: "" } : {}),
    }));
    await commit(next);
  }

  // ── drag to reorder ────────────────────────────────────────────────────────
  let dragFrom = $state<number | null>(null);
  let dragOver = $state<number | null>(null);

  function onDragStart(e: DragEvent, i: number) {
    dragFrom = i;
    // Firefox needs data set for the drag to start at all.
    e.dataTransfer?.setData("text/plain", String(i));
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
  }
  function onDragOver(e: DragEvent, i: number) {
    if (dragFrom === null) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    dragOver = i;
  }
  async function onDrop(e: DragEvent, to: number) {
    e.preventDefault();
    const from = dragFrom;
    dragFrom = null;
    dragOver = null;
    if (from === null || from === to) return;
    const next = providers.map((p) => ({ name: p.name, baseUrl: p.baseUrl, model: p.model }));
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, moved);
    await commit(next);
  }
  function onDragEnd() {
    dragFrom = null;
    dragOver = null;
  }
</script>

<div class="wrap">
  {#if providers.length > 0}
    <ul class="list">
      {#each providers as p, i (p.name)}
        <li
          class="row"
          class:active={i === activeIdx}
          class:over={dragOver === i && dragFrom !== i}
          class:dragging={dragFrom === i}
          draggable="true"
          ondragstart={(e) => onDragStart(e, i)}
          ondragover={(e) => onDragOver(e, i)}
          ondrop={(e) => onDrop(e, i)}
          ondragend={onDragEnd}
        >
          <span class="handle" title="拖曳調整順序">⠿</span>
          <button class="pick" onclick={() => cmd.setTranslateProvider(i)} title="改用這一個">
            <span class="name">
              {p.name}
              {#if p.keySource === "env"}<span class="tag" title=".env 提供金鑰">env</span>{/if}
            </span>
            <span class="model">{p.model}</span>
          </button>
          {#if i === activeIdx}<span class="badge">使用中</span>{/if}
          <button class="mini" onclick={() => openEdit(i)} title="編輯">✎</button>
          <button class="mini del" onclick={() => remove(i)} title="移除" disabled={busy}>✕</button>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="hint warn">尚未設定任何翻譯供應商。按下方「新增」填入名稱與 API 金鑰。</p>
  {/if}

  {#if !adding && editing === null}
    <div class="actions">
      <button class="add" onclick={openAdd}>＋ 新增供應商</button>
      <span class="hint inline">由上往下備援，連續失敗兩次自動換下一個</span>
    </div>
  {/if}

  {#if adding || editing !== null}
    <div class="form">
      <div class="frow">
        <span class="flabel">名稱</span>
        <input
          class="fin" list="tl-presets" spellcheck="false" placeholder="groq"
          value={form.name} oninput={(e) => onNameInput(e.currentTarget.value)} />
      </div>
      <datalist id="tl-presets">
        {#each presets as name (name)}<option value={name}></option>{/each}
      </datalist>

      <div class="frow">
        <span class="flabel">Base URL</span>
        <input
          class="fin" spellcheck="false" placeholder="留空＝用內建預設"
          bind:value={form.baseUrl} />
      </div>
      <div class="frow">
        <span class="flabel">模型</span>
        <input
          class="fin" spellcheck="false" placeholder="留空＝用內建預設"
          bind:value={form.model} />
      </div>
      <div class="frow">
        <span class="flabel">API 金鑰</span>
        <input
          class="fin" type="password" autocomplete="off" spellcheck="false"
          placeholder={editing !== null ? "留空＝不更動" : "貼上你的金鑰"}
          bind:value={keyDraft} />
      </div>

      {#if error}<p class="hint err">{error}</p>{/if}

      <div class="frow buttons">
        <button class="ok" onclick={submit} disabled={busy}>
          {busy ? "儲存中…" : editing !== null ? "儲存" : "新增"}
        </button>
        <button class="cancel" onclick={cancel} disabled={busy}>取消</button>
        {#if editing !== null && providers[editing]?.keySource === "settings"}
          <button class="cancel" onclick={() => clearKey(editing!)} disabled={busy}
                  title="清除後改用 .env 的金鑰">清除金鑰</button>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .wrap { padding: 2px 14px 4px; }

  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 4px;
    background: #16202c;
    border: 1px solid #2c3a4a;
    border-radius: 6px;
    padding: 3px 6px 3px 2px;
  }
  .row.active { background: #16302a; border-color: #2f6f52; }
  .row.dragging { opacity: 0.4; }
  /* The drop target, marked on its top edge so the insertion point is obvious. */
  .row.over { border-top-color: #7bcfa0; box-shadow: inset 0 2px 0 #7bcfa0; }

  .handle {
    flex-shrink: 0;
    width: 16px;
    text-align: center;
    color: #46525f;
    cursor: grab;
    font-size: 12px;
    line-height: 1;
    user-select: none;
  }
  .handle:active { cursor: grabbing; }

  .pick {
    display: flex;
    flex-direction: column;
    gap: 1px;
    align-items: flex-start;
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    padding: 3px 2px;
    color: #b7c2ce;
    font-family: inherit;
    cursor: pointer;
    text-align: left;
  }
  .row.active .pick { color: #cfe8dc; }
  .name { font-weight: 600; font-size: 12px; display: flex; align-items: center; gap: 5px; }
  .model {
    font-size: 10px;
    color: #5e6b78;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row.active .model { color: #6f9284; }

  .tag {
    font-size: 8px;
    letter-spacing: 0.06em;
    color: #7a869a;
    border: 1px solid #39434f;
    border-radius: 3px;
    padding: 0 3px;
    font-weight: 500;
  }

  .badge {
    flex-shrink: 0;
    font-size: 9px;
    letter-spacing: 0.06em;
    color: #7bcfa0;
    border: 1px solid #2f6f52;
    border-radius: 4px;
    padding: 1px 5px;
  }

  .mini {
    flex-shrink: 0;
    width: 20px;
    height: 20px;
    padding: 0;
    background: none;
    border: 1px solid transparent;
    border-radius: 4px;
    color: #6d7987;
    cursor: pointer;
    font-size: 11px;
    font-family: inherit;
  }
  .mini:hover { background: #22303f; border-color: #35455a; color: #cfd8e3; }
  .mini.del:hover { background: #3a2020; border-color: #6a3030; color: #ffb0a0; }
  .mini:disabled { opacity: 0.4; cursor: default; }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-top: 7px;
  }
  .add {
    background: #22303f;
    border: 1px dashed #3a5591;
    color: #9fc0e8;
    border-radius: 6px;
    padding: 4px 10px;
    font-size: 11px;
    font-family: inherit;
    cursor: pointer;
    flex-shrink: 0;
  }
  .add:hover { background: #2a3d55; }

  .form {
    margin-top: 7px;
    padding: 8px;
    background: #131b25;
    border: 1px solid #2c3a4a;
    border-radius: 6px;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .frow { display: flex; align-items: center; gap: 8px; }
  .flabel { flex-shrink: 0; width: 62px; color: #9aa3ae; font-size: 11px; }
  .fin {
    flex: 1;
    min-width: 0;
    background: #16202c;
    border: 1px solid #2c3a4a;
    border-radius: 5px;
    padding: 4px 7px;
    color: #cfd8e3;
    font-size: 11px;
    font-family: inherit;
  }
  .fin:focus { outline: none; border-color: #3a5591; }
  .fin::placeholder { color: #4e5a65; }

  .buttons { padding-top: 2px; gap: 6px; }
  .ok {
    background: #2a3d6a; border: 1px solid #3a5591; color: #a0c8ff;
    border-radius: 6px; padding: 4px 14px; font-size: 11px;
    font-family: inherit; cursor: pointer;
  }
  .ok:hover { background: #334880; }
  .ok:disabled { opacity: 0.45; cursor: default; }
  .cancel {
    background: none; border: 1px solid #333d4a; color: #8a93a0;
    border-radius: 6px; padding: 4px 10px; font-size: 11px;
    font-family: inherit; cursor: pointer;
  }
  .cancel:hover { background: #22303f; color: #cfd8e3; }
  .cancel:disabled { opacity: 0.45; cursor: default; }

  .hint {
    margin: 0;
    font-size: 10px;
    color: #4e5a65;
    line-height: 1.45;
  }
  .hint.inline { flex: 1; min-width: 0; }
  .hint.warn { color: #a8705a; padding: 2px 0 4px; }
  .hint.err { color: #e08070; }
</style>
