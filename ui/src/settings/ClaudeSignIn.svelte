<script lang="ts">
  // Claude Code's own sign-in, started from Settings: a console window opens running
  // `claude auth login --claudeai`, Claude Code opens the browser for claude.ai, and this asks
  // `auth status` every 2 s, for up to 5 minutes and only while Settings is on screen, until it's
  // signed in. The console is the user's to close.
  import { onDestroy } from "svelte";
  import { api } from "./ipc";
  import { waitForSignIn } from "./signin";

  interface Props {
    profileId: string;
    onsignedin: () => void;
  }
  let { profileId, onsignedin }: Props = $props();

  let phase = $state<"idle" | "waiting" | "signed-in">("idle");
  let note = $state("");
  let cancelled = false;

  async function start() {
    note = "";
    cancelled = false;
    try {
      await api.claudeSignIn(profileId);
    } catch (e) {
      note = String(e);
      return;
    }
    phase = "waiting";
    const end = await waitForSignIn(async () => (await api.checkProvider(profileId)).ready, {
      every: 2000,
      limit: 5 * 60 * 1000,
      visible: () => !document.hidden,
      cancelled: () => cancelled,
    });
    if (end === "signed-in") {
      phase = "signed-in";
      onsignedin();
    } else {
      phase = "idle";
      if (end === "timed-out") note = "Stopped waiting after 5 minutes. Sign in again to keep going.";
    }
  }

  function cancel() {
    cancelled = true;
    phase = "idle";
  }

  onDestroy(() => (cancelled = true));
</script>

<div class="signin">
  {#if phase === "waiting"}
    <span class="waiting">Finish signing in in your browser…</span>
    <button class="btn small" onclick={cancel}>Cancel</button>
  {:else if phase === "signed-in"}
    <span class="done">✓ Signed in</span>
  {:else}
    <button class="btn small primary" onclick={start}>Sign in to Claude</button>
  {/if}
  {#if note}<span class="hint">{note}</span>{/if}
</div>

<style>
  .signin {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 10px;
  }
  /* Under a found account's warning, not inside a setup panel's own spacing. */
  :global(.found) .signin {
    margin-top: 6px;
  }
  .waiting {
    font-size: 12.5px;
    color: var(--text);
  }
  .done {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--good, var(--text));
  }
  .hint {
    flex-basis: 100%;
    font-size: 11.5px;
    color: var(--text-3, var(--text-2));
  }
</style>
