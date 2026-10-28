<script lang="ts">
  import type { Config } from "../lib/types";
  import ShortcutInput from "./ShortcutInput.svelte";

  interface Props {
    config: Config;
    problems: string[];
    onchange: () => void;
  }
  let { config = $bindable(), problems, onchange }: Props = $props();
  let newApp = $state("");

  function addApp() {
    const id = newApp.trim();
    if (id && !config.excluded_apps.includes(id)) {
      config.excluded_apps = [...config.excluded_apps, id];
      onchange();
    }
    newApp = "";
  }
</script>

<div class="page">
  <h1>Behavior</h1>
  <p class="lede">Shortcuts, suggestions, and where SuperEditr stays out of the way.</p>

  <div class="group-title">Keyboard</div>
  <div class="group">
    <div class="row">
      <div>
        <div class="label">Review selection</div>
        <div class="hint">Opens suggestions for the selected text. Press again to bring an automatic suggestion into focus. Use a key combination such as ⌃⌥E or F6, or a double press like ⌘E E (press it twice quickly). Shortcuts macOS or other apps already use are refused.</div>
      </div>
      <ShortcutInput slot="review" value={config.hotkeys.review} onchange={(v) => { config.hotkeys.review = v; onchange(); }} />
    </div>
    <div class="row">
      <div>
        <div class="label">Quick fix</div>
        <div class="hint">Applies your default style to the selection immediately, without showing suggestions.</div>
      </div>
      <ShortcutInput slot="quick_fix" value={config.hotkeys.quick_fix} onchange={(v) => { config.hotkeys.quick_fix = v; onchange(); }} />
    </div>
    <div class="row">
      <div>
        <div class="label">With nothing selected, use the sentence before the cursor</div>
        <div class="hint">The shortcuts select the sentence you just wrote, so you see what they’ll change, and review it. Where an app hides its text, they select back to the start of the paragraph. Automatic suggestions never do this.</div>
      </div>
      <input type="checkbox" class="switch" bind:checked={config.select_sentence} onchange={onchange} />
    </div>
    <div class="row keys">
      <div class="label">In the suggestion panel</div>
      <div class="cheats">
        <span><kbd>⌥↩</kbd> or <kbd>↩</kbd> replace</span>
        <span><kbd>tab</kbd> next change</span>
        <span><kbd>␣</kbd> keep original</span>
        <span><kbd>1</kbd>–<kbd>7</kbd> style</span>
        <span><kbd>/</kbd> instruct</span>
        <span><kbd>R</kbd> retry</span>
        <span><kbd>⌘C</kbd> copy</span>
      </div>
    </div>
    {#if problems.length}
      <div class="row warn">Some shortcuts are taken by another app: {problems.join("; ")}</div>
    {/if}
  </div>

  <div class="group-title">Suggestions</div>
  <div class="group">
    <div class="row">
      <div>
        <div class="label">Suggest as soon as I select text</div>
        <div class="hint">Shows suggestions automatically when you select text you’re editing. Typing or clicking elsewhere hides them. Turn off to use the shortcut only.</div>
      </div>
      <input type="checkbox" class="switch" bind:checked={config.auto_popup} onchange={onchange} />
    </div>
    <div class="row">
      <div>
        <div class="label">Minimum selection</div>
        <div class="hint">Shorter selections are ignored for automatic suggestions.</div>
      </div>
      <span><input type="number" min="1" max="500" bind:value={config.auto_min_chars} onchange={onchange} style="width: 70px" /> characters</span>
    </div>
    <div class="row">
      <div class="label">Show response times</div>
      <input type="checkbox" class="switch" bind:checked={config.show_timing} onchange={onchange} />
    </div>
  </div>

  <div class="group-title">Apps where SuperEditr never reads text</div>
  <div class="group">
    <div class="row">
      <input type="text" placeholder="Bundle ID, e.g. com.apple.Notes" bind:value={newApp} onkeydown={(e) => e.key === "Enter" && addApp()} spellcheck="false" />
      <button class="btn small" onclick={addApp} disabled={!newApp.trim()}>Add</button>
    </div>
    {#each config.excluded_apps as app (app)}
      <div class="row">
        <code>{app}</code>
        <button class="btn small" onclick={() => { config.excluded_apps = config.excluded_apps.filter((a) => a !== app); onchange(); }}>Remove</button>
      </div>
    {/each}
  </div>
</div>

<style>
  .hint {
    color: var(--text-3);
    font-size: 12px;
    margin-top: 2px;
    max-width: 480px;
    line-height: 1.4;
  }
  .label {
    font-weight: 500;
  }
  .keys {
    align-items: flex-start;
  }
  .cheats {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 14px;
    justify-content: flex-end;
    max-width: 420px;
    color: var(--text-2);
    font-size: 12px;
  }
  .warn {
    color: var(--warn);
    background: var(--warn-bg);
  }
  code {
    font-family: var(--mono);
    font-size: 12px;
  }
  .row input[type="text"] {
    flex: 1;
  }
</style>
