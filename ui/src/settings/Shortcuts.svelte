<script lang="ts">
  import type { Config } from "../lib/types";
  import ShortcutInput from "./ShortcutInput.svelte";
  import ShortcutPicker from "./ShortcutPicker.svelte";
  import { isLinux, isMac, isWindows, words } from "../lib/platform";

  interface Props {
    config: Config;
    problems: string[];
    onchange: () => void;
  }
  let { config = $bindable(), problems, onchange }: Props = $props();
  const w = words();
</script>

<div class="page">
  <h1>Shortcuts</h1>
  <p class="lede">Review the text you select from any app. Each style can also have a shortcut of its own, in Styles.</p>

  <div class="group">
    <div class="row">
      <div>
        <div class="label" id="review-shortcut-label">Review selection</div>
        {#if isMac()}
          <div class="hint" id="review-shortcut-hint">Opens suggestions for the selected text. Press again to bring an automatic suggestion into focus. Use a key combination such as ⌃⌥E or F6, or a double press like ⌘E E (press it twice quickly). Shortcuts macOS or other apps already use are refused.</div>
        {:else if isWindows()}
          <div class="hint" id="review-shortcut-hint">Opens suggestions for the selected text. Use a key combination such as Ctrl+Shift+F2 or Ctrl+Alt+Space. Shortcuts Windows or other apps already use are refused.</div>
        {:else}
          <div class="hint" id="review-shortcut-hint">Opens suggestions for the selected text. These combinations are free on Omarchy; one your desktop already uses can’t be chosen.</div>
        {/if}
      </div>
      {#if isLinux()}
        <ShortcutPicker slot="review" labelledby="review-shortcut-label" describedby="review-shortcut-hint" value={config.hotkeys.review} onchange={(v) => { config.hotkeys.review = v; onchange(); }} />
      {:else}
        <ShortcutInput slot="review" labelledby="review-shortcut-label" describedby="review-shortcut-hint" value={config.hotkeys.review} onchange={(v) => { config.hotkeys.review = v; onchange(); }} />
      {/if}
    </div>
    <div class="row">
      <div>
        <div class="label" id="quick-fix-label">Quick fix</div>
        <div class="hint" id="quick-fix-hint">Applies your default style to the selection immediately, without showing suggestions.</div>
      </div>
      {#if isLinux()}
        <ShortcutPicker slot="quick_fix" labelledby="quick-fix-label" describedby="quick-fix-hint" value={config.hotkeys.quick_fix} onchange={(v) => { config.hotkeys.quick_fix = v; onchange(); }} />
      {:else}
        <ShortcutInput slot="quick_fix" labelledby="quick-fix-label" describedby="quick-fix-hint" value={config.hotkeys.quick_fix} onchange={(v) => { config.hotkeys.quick_fix = v; onchange(); }} />
      {/if}
    </div>
    <div class="row">
      <div>
        <div class="label" id="select-sentence-label">With nothing selected, use the sentence before the cursor</div>
        <div class="hint" id="select-sentence-hint">The shortcuts select the sentence you just wrote, so you see what they’ll change, and review it. In an app that hides its text, select it yourself. Automatic suggestions never do this.</div>
      </div>
      <input type="checkbox" class="switch" aria-labelledby="select-sentence-label" aria-describedby="select-sentence-hint" bind:checked={config.select_sentence} onchange={onchange} />
    </div>
    <div class="row keys">
      <div class="label">In the suggestion panel</div>
      <div class="cheats">
        <span>{#each [...w.applyKeys].reverse() as key, i (key)}{#if i} or {/if}<kbd>{key}</kbd>{/each} replace</span>
        <span><kbd>{w.tabKey}</kbd> next change</span>
        <span><kbd>␣</kbd> keep original</span>
        <span><kbd>1</kbd>–<kbd>7</kbd> style</span>
        <span><kbd>/</kbd> instruct</span>
        <span><kbd>R</kbd> retry</span>
        <span><kbd>{w.copyKey}</kbd> copy</span>
      </div>
    </div>
    <div class="row">
      <div>
        <div class="label" id="always-show-tips-label">Always show key tips in the suggestion panel</div>
        <div class="hint" id="always-show-tips-hint">Off, the tips appear for your first few reviews, or until you close them.</div>
      </div>
      <input type="checkbox" class="switch" aria-labelledby="always-show-tips-label" aria-describedby="always-show-tips-hint" bind:checked={config.always_show_tips} onchange={onchange} />
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
