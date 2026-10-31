<script lang="ts">
  import { onMount } from "svelte";
  import type { Config, SettingsPayload } from "../lib/types";
  import Dictionary from "./Dictionary.svelte";
  import General from "./General.svelte";
  import Models from "./Models.svelte";
  import Privacy from "./Privacy.svelte";
  import Shortcuts from "./Shortcuts.svelte";
  import Styles from "./Styles.svelte";
  import Welcome from "./Welcome.svelte";
  import { api } from "./ipc";
  import { mergeUnsaved, saveUntilCurrent } from "./merge";

  type Section = "welcome" | "models" | "styles" | "dictionary" | "general" | "shortcuts" | "privacy" | "about";
  // The pages about the writing, then a gap, then the app's own.
  const nav: { id: Section; name: string; icon: string; gap?: boolean }[] = [
    { id: "welcome", name: "Get Started", icon: "✦" },
    { id: "models", name: "Models", icon: "◎" },
    { id: "styles", name: "Styles", icon: "¶" },
    { id: "dictionary", name: "Dictionary", icon: "Aa" },
    { id: "general", name: "General", icon: "⚙", gap: true },
    { id: "shortcuts", name: "Shortcuts", icon: "⌘" },
    { id: "privacy", name: "Privacy", icon: "⚿" },
    { id: "about", name: "About", icon: "ⓘ" },
  ];

  let payload = $state<SettingsPayload | null>(null);
  let config = $state<Config | null>(null);
  let section = $state<Section>("welcome");
  let problems = $state<string[]>([]);
  let saved = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  /** The config as Settings last loaded or saved it: what the user's unsaved changes are measured from. */
  let base: Config | null = null;
  /** The revision `base` was saved at: a save based on an older one is refused. */
  let revision = 0;
  /** Changes made here, and how many of them a finished save has reached. */
  let edits = 0;
  let savedEdits = 0;
  /** Saves run one at a time, each based on the revision the one before it made. */
  let saving: Promise<void> = Promise.resolve();
  /** Set by Get Started's "Find my AI accounts": Models scans as soon as it opens. */
  let scanOnOpen = $state(false);

  function findAccounts() {
    scanOnOpen = true;
    section = "models";
  }

  async function reload() {
    const fresh = await api.get();
    const latest = structuredClone(fresh.config);
    // Changes waiting to be saved, or still being saved, survive a reload.
    if (config && base && edits !== savedEdits) {
      fresh.config = mergeUnsaved(base, $state.snapshot(config) as Config, fresh.config);
    }
    base = latest;
    revision = fresh.revision;
    payload = fresh;
    config = payload.config;
  }

  function onchange() {
    edits++;
    clearTimeout(timer);
    timer = setTimeout(() => (saving = saving.then(() => save()).catch(console.error)), 250);
  }

  /** Saves the whole config. When the app has saved it since Settings loaded or saved it, the app
   * refuses, and Settings reloads, keeping its unsaved changes, and tries again. If the app keeps
   * refusing, the changes stay unsaved until the next one. */
  async function save(): Promise<void> {
    await saveUntilCurrent(async () => {
      if (!config || edits === savedEdits) return true;
      const reached = edits;
      const snapshot = $state.snapshot(config) as Config;
      const result = await api.save(snapshot, revision);
      if (result.revision === null) return false;
      problems = result.problems;
      revision = result.revision;
      base = snapshot;
      savedEdits = reached;
      saved = true;
      setTimeout(() => (saved = false), 1200);
      return true;
    }, reload);
  }

  function finish() {
    if (!config) return;
    config.onboarded = true;
    onchange();
    section = "models";
  }

  onMount(() => {
    reload().then(() => {
      if (payload?.config.onboarded) section = "models";
    });
    let stop: (() => void) | undefined;
    api.on<string>("settings:section", (s) => {
      if (s === "privacy") section = payload?.config.onboarded ? "privacy" : "welcome";
      else if (nav.some((n) => n.id === s)) section = s as Section;
      reload();
    }).then((s) => (stop = s));
    // A word added from the panel ("Always keep").
    let stopReload: (() => void) | undefined;
    api.on("settings:reload", () => reload()).then((s) => (stopReload = s));
    const refresh = () => reload();
    window.addEventListener("focus", refresh);
    return () => {
      stop?.();
      stopReload?.();
      window.removeEventListener("focus", refresh);
    };
  });
</script>

<div class="shell">
  <aside data-tauri-drag-region>
    <div class="brand" data-tauri-drag-region>
      <img src="/icon.png" alt="" width="28" height="28" />
      <span>SuperEditr</span>
    </div>
    <nav>
      {#each nav as item (item.id)}
        <button class:active={section === item.id} class:gap={item.gap} onclick={() => (section = item.id)}>
          <span class="icon" aria-hidden="true">{item.icon}</span>{item.name}
        </button>
      {/each}
    </nav>
    <div class="foot">
      {#if payload && !payload.accessibility}
        <button class="warn" onclick={() => (section = payload?.config.onboarded ? "privacy" : "welcome")}>Accessibility access needed</button>
      {/if}
      {#if saved}<span class="saved">Saved</span>{/if}
    </div>
  </aside>
  <main>
    <div class="drag" data-tauri-drag-region></div>
    {#if config && payload}
      {#if section === "welcome"}
        <Welcome {config} accessibility={payload.accessibility} {reload} {finish} openModels={() => (section = "models")} {findAccounts} />
      {:else if section === "models"}
        <Models
          bind:config
          keys={payload.keys}
          {onchange}
          {reload}
          bind:scanOnOpen
          backToSetup={config.onboarded ? null : () => (section = "welcome")}
        />
      {:else if section === "styles"}
        <Styles bind:config {onchange} />
      {:else if section === "dictionary"}
        <Dictionary bind:config {onchange} />
      {:else if section === "general"}
        <General bind:config {onchange} />
      {:else if section === "shortcuts"}
        <Shortcuts bind:config {problems} {onchange} />
      {:else if section === "privacy"}
        <Privacy bind:config accessibility={payload.accessibility} {onchange} />
      {:else}
        <div class="page">
          <h1>About SuperEditr</h1>
          <p class="lede">Version {payload.version}. Fast, private, keyboard-first proofreading that works in any app, with the AI you choose.</p>
          <div class="group">
            <div class="row"><span>The rule-based checker is <button class="linklike" onclick={() => api.openUrl("https://writewithharper.com")}>Harper</button> (Apache-2.0).</span></div>
          </div>
        </div>
      {/if}
    {/if}
  </main>
</div>

<style>
  .shell {
    display: grid;
    grid-template-columns: 210px 1fr;
    height: 100vh;
  }
  aside {
    background: var(--bg-sunken);
    border-right: 1px solid var(--border);
    padding: 40px 10px 12px;
    display: flex;
    flex-direction: column;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 650;
    font-size: 15px;
    padding: 0 8px 16px;
  }
  nav {
    display: grid;
    gap: 2px;
  }
  nav button {
    all: unset;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 10px;
    border-radius: 8px;
    cursor: pointer;
    font-weight: 500;
  }
  nav button:hover {
    background: var(--bg-hover);
  }
  nav button.gap {
    margin-top: 12px;
  }
  nav button.active {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .icon {
    width: 16px;
    text-align: center;
    opacity: 0.8;
  }
  .foot {
    margin-top: auto;
    display: grid;
    gap: 8px;
    padding: 0 6px;
  }
  .warn {
    all: unset;
    font-size: 12px;
    color: var(--warn);
    background: var(--warn-bg);
    padding: 6px 9px;
    border-radius: 8px;
    cursor: pointer;
  }
  .saved {
    font-size: 12px;
    color: var(--text-3);
  }
  main {
    overflow-y: auto;
    padding: 0 36px 40px;
    position: relative;
  }
  .drag {
    height: 36px;
    position: sticky;
    top: 0;
  }
  main :global(.page) {
    max-width: 720px;
  }
</style>
