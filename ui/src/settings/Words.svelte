<script lang="ts">
  import type { Config } from "../lib/types";

  interface Props {
    config: Config;
    onchange: () => void;
  }
  let { config = $bindable(), onchange }: Props = $props();
  let newWord = $state("");

  function addWord() {
    const word = newWord.trim();
    if (word && !config.words.some((w) => w.toLowerCase() === word.toLowerCase())) {
      config.words = [...config.words, word];
      onchange();
    }
    newWord = "";
  }

  function removeWord(word: string) {
    config.words = config.words.filter((w) => w !== word);
    onchange();
  }
</script>

<div class="page">
  <h1>Your words</h1>
  <p class="lede">
    Names, places, products, and addresses that SuperEditr keeps exactly as written. The rule-based checker treats them as
    correctly spelled, and your AI is told to keep the ones in the text you select. Add one here, or press <kbd>A</kbd> (Always
    keep) in the suggestion panel.
  </p>

  <div class="group">
    <div class="row column">
      <form class="add-word" onsubmit={(e) => { e.preventDefault(); addWord(); }}>
        <input type="text" bind:value={newWord} placeholder="e.g. Rhyan, or 1234 Glenmoor Ct" spellcheck="false" />
        <button class="btn small" type="submit" disabled={!newWord.trim()}>Add</button>
      </form>
      {#if config.words.length}
        <ul class="words">
          {#each config.words as word (word)}
            <li>{word}<button aria-label={`Remove ${word}`} title="Remove" onclick={() => removeWord(word)}>×</button></li>
          {/each}
        </ul>
      {:else}
        <p class="empty">No words yet.</p>
      {/if}
    </div>
  </div>
</div>

<style>
  .column {
    flex-direction: column;
    align-items: stretch;
  }
  .words {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .words li {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 4px 2px 10px;
    border-radius: 999px;
    background: var(--bg-sunken);
    border: 1px solid var(--border);
  }
  .words button {
    all: unset;
    padding: 0 6px;
    color: var(--text-3);
    cursor: pointer;
  }
  .words button:hover {
    color: var(--text);
  }
  .add-word {
    display: flex;
    gap: 8px;
  }
  .add-word input {
    flex: 1;
  }
  .empty {
    margin: 0;
    color: var(--text-3);
    font-size: 12px;
  }
</style>
