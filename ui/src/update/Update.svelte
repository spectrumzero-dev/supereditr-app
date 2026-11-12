<script lang="ts">
  // Opens as soon as someone chooses Check for Updates…, and stays until they close it, so the result
  // never sits unseen in a menu that has already closed. The window changes in place and never takes
  // focus back, so a result can't catch someone typing elsewhere with Return on Restart Now.
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import type { UpdateStatus } from "../lib/types";
  import { isWindows } from "../lib/platform";

  interface View {
    heading: string;
    detail?: string;
    /** A bar's percent, or null for one that just shows activity. No bar when absent. */
    progress?: number | null;
    /** The main button's words, when there's something to do. */
    action?: string;
    dismiss: string;
  }

  function viewOf(status: UpdateStatus): View {
    switch (status.state) {
      case "idle":
      case "checking":
        return { heading: "Checking for updates…", progress: null, dismiss: "Close" };
      case "downloading":
        return {
          heading: `Downloading SuperEditr ${status.version}…`,
          // On Windows the update installs by itself once SuperEditr is idle (updater.rs).
          detail: isWindows()
            ? "You can close this window. SuperEditr installs the update while you’re not using it."
            : "You can close this window. The menu-bar menu offers the restart when it’s ready.",
          progress: status.percent,
          dismiss: "Close",
        };
      case "ready":
        return {
          heading: `SuperEditr ${status.version} is ready`,
          detail: isWindows()
            ? "SuperEditr installs it while you’re not using it, and opens again afterwards. Or restart now to install it."
            : "Restart SuperEditr to start using it. You can also restart later from the menu-bar menu.",
          action: "Restart Now",
          dismiss: "Later",
        };
      case "up-to-date":
        return { heading: "You’re up to date", detail: `SuperEditr ${status.version} is the newest version.`, dismiss: "OK" };
      case "check-failed":
        return {
          heading: "Couldn’t check for updates",
          detail: "Check your internet connection and try again.",
          action: "Try Again",
          dismiss: "Close",
        };
      case "install-failed":
        return {
          heading: `Couldn’t install SuperEditr ${status.version}`,
          detail: "The version you have still works. You can try again.",
          action: "Try Again",
          dismiss: "Close",
        };
    }
  }

  let status = $state<UpdateStatus>({ state: "checking" });
  const view = $derived(viewOf(status));

  const act = () => invoke("update_now");
  const close = () => invoke("close_update_window");

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      close();
    } else if (e.key === "Enter" && !(document.activeElement instanceof HTMLButtonElement)) {
      // A button someone tabbed to answers Return itself.
      e.preventDefault();
      if (view.action) act();
      else close();
    }
  }

  onMount(() => {
    // Listening first, so nothing that happens during the read is missed. Anything heard is newer
    // than the read, whichever arrives first.
    let heard = false;
    const stop = listen<UpdateStatus>("update:status", (e) => {
      heard = true;
      status = e.payload;
    });
    stop
      .then(() => invoke<UpdateStatus>("update_status"))
      .then((now) => {
        if (!heard) status = now;
      });
    return () => void stop.then((unlisten) => unlisten());
  });
</script>

<svelte:window onkeydown={onKey} />

<main>
  <img src="/icon.png" alt="" width="48" height="48" draggable="false" />
  <div class="body">
    <h1>{view.heading}</h1>
    {#if view.detail}<p>{view.detail}</p>{/if}
    {#if view.progress !== undefined}<progress value={view.progress ?? undefined} max="100"></progress>{/if}
    <div class="buttons">
      <button class="btn" class:primary={!view.action && view.progress === undefined} onclick={close}>{view.dismiss}</button>
      {#if view.action}<button class="btn primary" onclick={act}>{view.action}</button>{/if}
    </div>
  </div>
</main>

<style>
  :global(body.update-body) {
    background: var(--bg);
    overflow: hidden;
    user-select: none;
    -webkit-user-select: none;
    cursor: default;
  }
  main {
    display: flex;
    gap: 16px;
    height: 100vh;
    padding: 20px;
  }
  .body {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }
  p {
    margin: 0;
    line-height: 1.4;
    color: var(--text-2);
  }
  progress {
    flex-shrink: 0;
    width: 100%;
    margin-top: 4px;
    accent-color: var(--accent);
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: auto;
  }
  .btn {
    border-radius: 8px;
    padding: 5px 12px;
    border: 1px solid var(--border-strong);
    background: var(--bg);
    cursor: pointer;
    font-weight: 500;
  }
  .btn:hover {
    background: var(--bg-hover);
  }
  .btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-text);
  }
  .btn.primary:hover {
    filter: brightness(1.08);
  }
</style>
