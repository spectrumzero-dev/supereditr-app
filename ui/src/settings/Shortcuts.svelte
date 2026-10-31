<script lang="ts">
  import type { Config } from "../lib/types";
  import ShortcutInput from "./ShortcutInput.svelte";

  interface Props {
    config: Config;
    problems: string[];
    onchange: () => void;
  }
  let { config = $bindable(), problems, onchange }: Props = $props();
</script>

<div class="page">
  <h1>Shortcuts</h1>
  <p class="lede">Review the text you select from any app. Each style can also have a shortcut of its own, in Styles.</p>

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
        <div class="hint">The shortcuts select the sentence you just wrote, so you see what they’ll change, and review it. In an app that hides its text, select it yourself. Automatic suggestions never do this.</div>
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
</style>
