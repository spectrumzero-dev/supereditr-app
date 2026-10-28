<script lang="ts">
  // The keys for reviewing, as large keycaps: a reference. The panel's own keys are learned in
  // the panel (it shows tips for the first few reviews), and Try it now tests ⌘E E for real.
  import { keycaps } from "../lib/accelerator";
  import { keySymbols } from "../lib/diff";

  interface Props {
    review: string;
    /** Suggestions also appear on their own after a selection. */
    auto: boolean;
  }
  let { review, auto }: Props = $props();

  const items = $derived([
    { id: "review", caps: keycaps(review), label: "Review selection" },
    { id: "apply", caps: ["↩"], label: "Apply" },
    // Spelled out: the ⇥ symbol reads as an arrow.
    { id: "next", caps: ["tab"], label: "Next change" },
    { id: "styles", caps: ["1–7"], label: "Switch style" },
    { id: "close", caps: ["esc"], label: "Close" },
  ]);
</script>

<div class="shortcuts">
  <div class="keys">
    {#each items as item (item.id)}
      <div class="item">
        <div class="caps">{#each item.caps as cap, i (i)}<kbd class="cap">{cap}</kbd>{/each}</div>
        <div class="what">{item.label}</div>
      </div>
    {/each}
  </div>
  <p class="foot">
    Select text anywhere, then press {keySymbols(review)} to review it with these keys.{#if auto}
      Or just wait a moment: a suggestion appears on its own, and <kbd>⌥↩</kbd> applies it (↩ stays with your app).{/if}
  </p>
</div>

<style>
  /* Tiles share the row but never shrink below their keycaps and label. */
  .keys {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .keys > .item {
    flex: 1 0 max-content;
    max-width: 170px;
  }
  .item {
    display: grid;
    justify-items: center;
    gap: 6px;
    padding: 8px 10px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--bg-sunken);
  }
  .caps {
    display: flex;
    gap: 4px;
  }
  .cap {
    min-width: 30px;
    height: 30px;
    padding: 0 8px;
    display: grid;
    place-items: center;
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
    background: var(--bg-raised);
    border: 1px solid var(--border-strong);
    border-bottom-width: 3px;
    border-radius: 7px;
  }
  .what {
    font-size: 12px;
    color: var(--text-2);
    white-space: nowrap;
  }
  .foot {
    margin: 8px 0 0;
    font-size: 12.5px;
    color: var(--text-2);
  }
</style>
