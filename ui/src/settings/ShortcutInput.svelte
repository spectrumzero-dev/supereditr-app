<script lang="ts">
  import { onMount } from "svelte";
  import { acceleratorFromEvent } from "../lib/accelerator";
  import { keySymbols } from "../lib/diff";
  import { api } from "./ipc";

  interface Props {
    value: string;
    /** Which shortcut this is ("review", "quick_fix", "preset:<id>"), so it isn't reported as
     * conflicting with itself. */
    slot: string;
    onchange: (value: string) => void;
    placeholder?: string;
  }
  let { value, slot, onchange, placeholder = "Record shortcut" }: Props = $props();
  /** Why the last shortcut typed was refused; the previous one stays. */
  let problem = $state("");
  let recording = $state(false);
  let button: HTMLButtonElement | undefined = $state();
  /** The first chord, waiting briefly to see if it's pressed again (a double press like ⌘E E). */
  let first = $state<string | null>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const DOUBLE_PRESS_MS = 300;

  function setRecording(on: boolean) {
    recording = on;
    if (on) problem = "";
    first = null;
    clearTimeout(timer);
    // While recording, SuperEditr's own double-press watcher must let the keys through.
    api.pauseShortcuts(on);
  }

  async function finish(accelerator: string) {
    setRecording(false);
    problem = "";
    if (accelerator) {
      const conflict = await api.checkShortcut(accelerator, slot);
      if (conflict) {
        problem = `${keySymbols(accelerator)}: ${conflict}`;
        return;
      }
    }
    onchange(accelerator);
  }

  // Clicking a button doesn't focus it in macOS's web view, so keys are caught on the window while
  // recording. Esc, a click elsewhere, or leaving the window cancels.
  $effect(() => {
    if (!recording) return;
    button?.focus();
    const away = (e: PointerEvent) => !button?.contains(e.target as Node) && setRecording(false);
    const cancel = () => setRecording(false);
    window.addEventListener("keydown", onkeydown, true);
    window.addEventListener("pointerdown", away, true);
    window.addEventListener("blur", cancel);
    return () => {
      window.removeEventListener("keydown", onkeydown, true);
      window.removeEventListener("pointerdown", away, true);
      window.removeEventListener("blur", cancel);
    };
  });

  // Leaving the page mid-recording must not leave SuperEditr's shortcuts paused.
  onMount(() => () => {
    if (recording) api.pauseShortcuts(false);
    clearTimeout(timer);
  });

  function onkeydown(e: KeyboardEvent) {
    if (!recording) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.repeat) return;
    if (e.key === "Escape") {
      setRecording(false);
      return;
    }
    if (e.key === "Backspace" || e.key === "Delete") {
      finish("");
      return;
    }
    const accelerator = acceleratorFromEvent(e);
    if (!accelerator) {
      // A plain key would fire whenever it's typed; modifier keys alone are just the start.
      if (!["Shift", "Control", "Alt", "Meta", "CapsLock", "Fn"].includes(e.key)) {
        problem = "Add ⌘, ⌃, or ⌥ to that key, or use a function key such as F6.";
      }
      return;
    }
    problem = "";
    clearTimeout(timer);
    // Doubling needs a letter or digit (see src-tauri/src/double_press.rs).
    if (first === accelerator && /\+[A-Z0-9]$/.test(accelerator)) {
      finish(`${accelerator} ${accelerator}`);
      return;
    }
    first = accelerator;
    timer = setTimeout(() => first && finish(first), DOUBLE_PRESS_MS + 100);
  }
</script>

<span class="wrap">
  <button class="shortcut" class:recording bind:this={button} onclick={() => setRecording(!recording)}>
    {#if recording}
      {first ? `${keySymbols(first)} … press again for a double press` : "Type shortcut…"}
    {:else if value}
      {keySymbols(value)}
    {:else}
      <span class="muted">{placeholder}</span>
    {/if}
  </button>
  {#if problem}<span class="problem" role="alert">{problem}</span>{/if}
</span>

<style>
  .wrap {
    display: inline-flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 6px;
  }
  .problem {
    max-width: 300px;
    font-size: 12px;
    line-height: 1.4;
    color: var(--warn);
    text-align: right;
  }
  .shortcut {
    min-width: 120px;
    padding: 5px 10px;
    border-radius: 8px;
    border: 1px solid var(--border-strong);
    background: var(--bg-sunken);
    font-size: 13px;
    font-weight: 600;
    letter-spacing: 0.04em;
    cursor: pointer;
    text-align: center;
  }
  .recording {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
    color: var(--accent);
    letter-spacing: 0;
    font-weight: 500;
  }
  .muted {
    font-weight: 400;
    letter-spacing: 0;
  }
</style>
