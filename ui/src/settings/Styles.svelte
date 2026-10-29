<script lang="ts">
  import type { Config, Preset } from "../lib/types";
  import ShortcutInput from "./ShortcutInput.svelte";
  import Playground from "./Playground.svelte";
  import { api } from "./ipc";
  import { describeLosses, isDefault, losesWork, resetLosses } from "./styles";

  interface Props {
    config: Config;
    onchange: () => void;
  }
  let { config = $bindable(), onchange }: Props = $props();
  let selected = $state(config.presets[0]?.id ?? "");
  const preset = $derived(config.presets.find((p) => p.id === selected));

  const dialects = ["American English", "British English", "Canadian English", "Australian English", "Indian English"];

  function addPreset() {
    const id = `custom-${Date.now().toString(36)}`;
    const next: Preset = { id, name: "My style", instruction: "Rewrite the text so it ", builtin: false, shortcut: null };
    config.presets = [...config.presets, next];
    selected = id;
    onchange();
  }

  /** Any style can be deleted except the last one; Reset to default brings built-ins back. */
  function removePreset(id: string) {
    if (config.presets.length <= 1) return;
    const i = config.presets.findIndex((p) => p.id === id);
    config.presets = config.presets.filter((p) => p.id !== id);
    if (config.default_preset === id) config.default_preset = config.presets[0].id;
    selected = config.presets[Math.min(i, config.presets.length - 1)].id;
    onchange();
  }

  let defaults = $state<Preset[]>([]);
  api.defaultPresets().then((d) => (defaults = d ?? []));
  const losses = $derived(resetLosses(config.presets, defaults));
  const atDefaults = $derived(defaults.length > 0 && isDefault(config.presets, defaults));
  /** Asking before a reset that would throw away the user's work. */
  let confirming = $state(false);

  function askReset() {
    if (losesWork(losses)) confirming = true;
    else reset();
  }

  function reset() {
    confirming = false;
    config.presets = defaults.map((d) => ({ ...d }));
    if (!config.presets.some((p) => p.id === config.default_preset)) config.default_preset = config.presets[0].id;
    selected = config.presets[0].id;
    onchange();
  }

  function move(id: string, step: number) {
    const list = [...config.presets];
    const i = list.findIndex((p) => p.id === id);
    const j = i + step;
    if (j < 0 || j >= list.length) return;
    [list[i], list[j]] = [list[j], list[i]];
    config.presets = list;
    onchange();
  }
</script>

<div class="page">
  <h1>Styles</h1>
  <p class="lede">Styles are the instructions SuperEditr gives the model. The first seven appear in the overlay, where <kbd>1</kbd>–<kbd>7</kbd> switch between them. Edit, reorder, or delete them, or write your own.</p>

  <div class="group-title">Your voice</div>
  <div class="group">
    <div class="row column">
      <div>
        <div class="label">Standing instructions</div>
        <div class="hint">Applied to every style. Describe your audience, words to avoid, formatting habits, or paste a short sample of your writing.</div>
      </div>
      <textarea rows="3" bind:value={config.style_notes} onchange={onchange} placeholder="e.g. I write to engineers and executives. Keep sentences short. Never use em dashes. Prefer “use” over “utilize”."></textarea>
    </div>
    <div class="row">
      <div class="label">English spelling</div>
      <select bind:value={config.dialect} onchange={onchange}>
        {#each dialects as d (d)}<option>{d}</option>{/each}
      </select>
    </div>
    <div class="row">
      <div>
        <div class="label">Default style</div>
        <div class="hint">Used for automatic suggestions and quick fix.</div>
      </div>
      <select bind:value={config.default_preset} onchange={onchange}>
        {#each config.presets as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
      </select>
    </div>
  </div>

  <div class="group-title">Styles</div>
  <div class="editor group">
    <ul class="list">
      {#each config.presets as p, i (p.id)}
        <li>
          <button class:selected={p.id === selected} onclick={() => (selected = p.id)}>
            <span class="num">{i < 7 ? i + 1 : ""}</span>{p.name}
          </button>
        </li>
      {/each}
      <li><button class="add" onclick={addPreset}>+ New style <span class="pill early" title="Included in early access">Early access</span></button></li>
    </ul>
    {#if preset}
      <div class="detail">
        <label>Name <input type="text" bind:value={preset.name} onchange={onchange} /></label>
        <label class="grow">
          Instruction
          <textarea rows="6" bind:value={preset.instruction} onchange={onchange}></textarea>
        </label>
        <div class="line">
          <span>
            <span class="label">Shortcut</span>
            <span class="hint">Runs this style on the selection from any app.</span>
          </span>
          <ShortcutInput slot={`preset:${preset.id}`} value={preset.shortcut ?? ""} onchange={(v) => { preset.shortcut = v || null; onchange(); }} placeholder="None" />
        </div>
        <div class="line">
          <span class="actions">
            <button class="btn small" onclick={() => move(preset.id, -1)}>Move up</button>
            <button class="btn small" onclick={() => move(preset.id, 1)}>Move down</button>
          </span>
          <button
            class="btn small danger"
            onclick={() => removePreset(preset.id)}
            disabled={config.presets.length <= 1}
            title={config.presets.length <= 1 ? "Keep at least one style." : undefined}>Delete style</button>
        </div>
      </div>
    {/if}
  </div>

  <div class="reset">
    {#if confirming}
      <div class="confirm" role="alertdialog" aria-label="Reset styles to default">
        <p><strong>Reset styles to default?</strong> You’ll lose {describeLosses(losses)}. This can’t be undone. Your standing instructions stay.</p>
        <span class="actions">
          <button class="btn small" onclick={() => (confirming = false)}>Cancel</button>
          <button class="btn small danger" onclick={reset}>Reset</button>
        </span>
      </div>
    {:else}
      <button class="btn small" onclick={askReset} disabled={atDefaults || !defaults.length}>Reset to default</button>
    {/if}
  </div>

  <div class="group-title">Try it</div>
  <div class="group pad"><Playground {config} /></div>
</div>

<style>
  .column {
    flex-direction: column;
    align-items: stretch;
  }
  .hint {
    display: block;
    color: var(--text-3);
    font-size: 12px;
    margin-top: 2px;
  }
  .label {
    font-weight: 500;
  }
  .editor {
    display: grid;
    grid-template-columns: 190px 1fr;
    min-height: 300px;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 8px;
    border-right: 1px solid var(--border);
    background: var(--bg-sunken);
  }
  .list button {
    all: unset;
    display: flex;
    gap: 8px;
    width: 100%;
    box-sizing: border-box;
    padding: 6px 10px;
    border-radius: 7px;
    cursor: pointer;
  }
  .list button:hover {
    background: var(--bg-hover);
  }
  .list button.selected {
    background: var(--accent);
    color: var(--accent-text);
  }
  .list .num {
    width: 12px;
    opacity: 0.55;
    font-size: 11px;
    padding-top: 1px;
  }
  .list .add {
    color: var(--accent);
    flex-wrap: wrap;
    align-items: center;
  }
  .detail {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 14px;
  }
  .detail label {
    display: grid;
    gap: 6px;
    font-weight: 500;
  }
  .line {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
  }
  .actions {
    display: flex;
    gap: 6px;
  }
  .reset {
    display: flex;
    justify-content: flex-end;
    margin: -8px 0 18px;
  }
  .confirm {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 10px 14px;
    border-radius: 10px;
    background: var(--warn-bg);
    color: var(--text);
  }
  .confirm p {
    margin: 0;
    font-size: 12.5px;
  }
  .pad {
    padding: 14px;
  }
</style>
