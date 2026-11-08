<script lang="ts">
  import { onMount } from "svelte";
  import { keySymbols } from "../lib/diff";
  import { api } from "./ipc";

  interface Props {
    value: string;
    /** "review" or "quick_fix". */
    slot: string;
    onchange: (value: string) => void;
  }
  let { value, slot, onchange }: Props = $props();

  /** Combinations Omarchy 4 leaves free (Super+Alt+Shift+E is its "New email"). Linux has no
   * shortcut recorder yet, so these are the choices. */
  const CHOICES: Record<string, string[]> = {
    review: ["Super+Alt+E", "Super+Alt+R", "Super+Alt+P", "Super+Alt+C", "F6"],
    quick_fix: ["Super+Control+Alt+E", "Super+Control+Alt+P", "Super+Control+Alt+C", "F7"],
  };
  const choices = $derived(CHOICES[slot] ?? []);
  /** Why a choice can't be used, by choice. */
  let taken = $state<Record<string, string>>({});

  onMount(async () => {
    for (const choice of choices) {
      const problem = await api.checkShortcut(choice, slot);
      if (problem && choice !== value) taken[choice] = problem;
    }
  });
</script>

<select {value} onchange={(e) => onchange(e.currentTarget.value)} aria-label="Shortcut">
  {#each choices as choice (choice)}
    <option value={choice} disabled={choice in taken}>{keySymbols(choice, false)}{choice in taken ? ` — ${taken[choice]}` : ""}</option>
  {/each}
  {#if value && !choices.includes(value)}
    <option {value}>{keySymbols(value, false)}</option>
  {/if}
  <option value="">Off</option>
</select>

<style>
  select {
    font: inherit;
    padding: 5px 8px;
    border-radius: 8px;
    border: 1px solid var(--border-strong);
    background: var(--bg-raised);
    color: var(--text);
    max-width: 260px;
  }
</style>
