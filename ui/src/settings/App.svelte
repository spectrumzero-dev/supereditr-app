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
  import LinuxSetup from "./LinuxSetup.svelte";
  import { api } from "./ipc";
  import { mergeUnsaved, needsRepairBeforeMerge, saveUntilCurrent } from "./merge";
  import { isLinux, isMac, isWindows, setKeyLabels, setPlatform } from "../lib/platform";

  type Section = "welcome" | "setup" | "models" | "styles" | "dictionary" | "general" | "shortcuts" | "privacy" | "about" | "licences";
  // The pages about the writing, then a gap, then the app's own.
  const nav: { id: Section; name: string; icon: string; gap?: boolean }[] = [
    { id: "welcome", name: "Get Started", icon: "✦" },
    { id: "models", name: "Models", icon: "◎" },
    { id: "styles", name: "Styles", icon: "¶" },
    { id: "dictionary", name: "Dictionary", icon: "Aa" },
    { id: "general", name: "General", icon: "⚙", gap: true },
    ...(isLinux() ? [{ id: "setup" as Section, name: "Desktop setup", icon: "▣" }] : []),
    { id: "shortcuts", name: "Shortcuts", icon: isMac() ? "⌘" : "⌨" },
    { id: "privacy", name: "Privacy", icon: "⚿" },
    { id: "about", name: "About", icon: "ⓘ" },
  ];

  let payload = $state<SettingsPayload | null>(null);
  let config = $state<Config | null>(null);
  let section = $state<Section>("welcome");
  let problems = $state<string[]>([]);
  let saved = $state(false);
  let failure = $state("");
  let retrying = $state(false);
  let waitingForRepair = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  /** The config as Settings last loaded or saved it: what the user's unsaved changes are measured from. */
  let base: Config | null = null;
  /** The revision `base` was saved at: a save based on an older one is refused. */
  let revision = 0;
  /** Changes made here, and how many of them a finished save has reached. */
  let edits = 0;
  let savedEdits = 0;
  /** Apply save and reload responses in order so a late response can't replace a newer base. */
  let pending: Promise<unknown> = Promise.resolve();
  /** Set by Get Started's "Find my AI accounts": Models scans as soon as it opens. */
  let scanOnOpen = $state(false);

  function findAccounts() {
    scanOnOpen = true;
    section = "models";
  }

  function enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const done = pending.then(operation);
    pending = done.catch(() => {});
    return done;
  }

  async function load(reread = false) {
    const fresh = await api.get(reread);
    setPlatform(fresh.platform);
    setKeyLabels(fresh.keyLabels ?? {});
    const latest = structuredClone(fresh.config);
    waitingForRepair = false;
    // Changes waiting to be saved, or still being saved, survive a reload.
    if (config && base && edits !== savedEdits) {
      const unsaved = $state.snapshot(config) as Config;
      if (needsRepairBeforeMerge(base, unsaved, fresh)) {
        waitingForRepair = true;
        failure = "Your changes are still pending. Repair the affected settings and select Retry.";
        payload = fresh;
        return;
      }
      fresh.config = mergeUnsaved(base, unsaved, fresh.config);
    }
    base = latest;
    revision = fresh.revision;
    problems = fresh.hotkeyProblems ?? [];
    payload = fresh;
    config = payload.config;
  }

  function reload() {
    return enqueue(async () => {
      try {
        await load();
      } catch (error) {
        failure = `Couldn’t load settings. ${errorMessage(error)}`;
      }
    });
  }

  function errorMessage(error: unknown) {
    return error instanceof Error ? error.message : String(error);
  }

  function settingsWriteFailed(message: string, error: unknown) {
    saved = false;
    failure = `${message} ${errorMessage(error)}`;
  }

  function onchange() {
    edits++;
    clearTimeout(timer);
    timer = setTimeout(() => void enqueue(save), 250);
  }

  /** Saves now, in turn with any other save, for a step that needs the change saved first (checking
   * an AI's new address). Settled once that save is: false when the app kept refusing it, so the
   * change is still unsaved. */
  function saveNow(): Promise<boolean> {
    edits++;
    clearTimeout(timer);
    return enqueue(save);
  }

  /** Saves the whole config. When the app has saved it since Settings loaded or saved it, the app
   * refuses, and Settings reloads, keeping its unsaved changes, and tries again. If the app keeps
   * refusing, the changes stay unsaved until the next one, and this returns false. */
  async function save(): Promise<boolean> {
    saved = false;
    try {
      const done = await saveUntilCurrent(async () => {
        if (!config || edits === savedEdits) return true;
        if (waitingForRepair || payload?.recovery?.writable === false) return false;
        const reached = edits;
        const snapshot = $state.snapshot(config) as Config;
        const result = await api.save(snapshot, revision);
        if (result.revision === null) return false;
        if (payload) payload.recovery = result.recovery;
        problems = result.problems;
        revision = result.revision;
        base = snapshot;
        savedEdits = reached;
        saved = true;
        setTimeout(() => (saved = false), 1200);
        return true;
      }, load);
      failure = done ? "" : waitingForRepair
        ? "Your changes are still pending. Repair the affected settings and select Retry."
        : "Couldn’t save your changes. Try again.";
      return done;
    } catch (error) {
      settingsWriteFailed("Couldn’t save your changes.", error);
      return false;
    }
  }

  function retry() {
    retrying = true;
    clearTimeout(timer);
    void enqueue(async () => {
      try {
        await load(true);
        await save();
      } catch (error) {
        failure = `Couldn’t load settings. ${errorMessage(error)}`;
      } finally {
        retrying = false;
      }
    });
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
    api.on("settings:section", (s) => {
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
      clearTimeout(timer);
      stop?.();
      stopReload?.();
      window.removeEventListener("focus", refresh);
    };
  });

  // On macOS a click anywhere closes the review panel, Settings included. Windows closes it only
  // when another app comes to the front, so a click in Settings (Set up AI under Try it now, say)
  // has to close it here.
  function closeReview() {
    if (isWindows()) api.dismissReview();
  }
</script>

<svelte:window onpointerdown={closeReview} />

<div class="shell">
  <aside data-tauri-drag-region>
    <div class="brand" data-tauri-drag-region>
      <img src="/icon.png" alt="" width="28" height="28" />
      <span>SuperEditr</span>
    </div>
    <nav>
      {#each nav as item (item.id)}
        <button class:active={section === item.id || (section === "licences" && item.id === "about")} class:gap={item.gap} onclick={() => (section = item.id)}>
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
    {#if payload?.recovery || failure}
      <section class="recovery" aria-label="Settings recovery">
        {#if payload?.recovery}
          {@const recovery = payload.recovery}
          <div role="status">
            {#if recovery.writable}
              <strong>Some settings need attention</strong>
              <p>Check these fields in the settings file. {recovery.issues.join(", ")}.</p>
              {#if recovery.issues.includes("providers")}
                <p>Unavailable AI profiles are kept in the file.</p>
              {/if}
            {:else}
              <strong>SuperEditr couldn’t load your settings</strong>
              <p>{recovery.issues.join(" ")}</p>
            {/if}
            {#if !recovery.exclusionsSafe}
              <p>Reviewing text from other apps is paused until your excluded apps can be read.</p>
            {/if}
            {#if !recovery.destinationSafe}
              <p>Reviews use on-device checks while the selected AI is unavailable.</p>
            {/if}
            <p>To reload a repaired file, select Retry.</p>
            <code>{recovery.path}</code>
          </div>
        {/if}
        {#if failure}<p role="alert">{failure}</p>{/if}
        <button class="btn small" disabled={retrying} onclick={retry}>Retry</button>
      </section>
    {/if}
    {#if config && payload && !retrying && !waitingForRepair && payload.recovery?.writable !== false}
      {#if section === "welcome"}
        <Welcome bind:config accessibility={payload.accessibility} {saveNow} {finish} openModels={() => (section = "models")} {findAccounts} />
      {:else if section === "models"}
        <Models
          bind:config
          keys={payload.keys}
          {saveNow}
          {reload}
          onWriteFailure={settingsWriteFailed}
          bind:scanOnOpen
          backToSetup={config.onboarded ? null : () => (section = "welcome")}
        />
      {:else if section === "styles"}
        <Styles bind:config {onchange} openModels={() => (section = "models")} />
      {:else if section === "dictionary"}
        <Dictionary bind:config {onchange} />
      {:else if section === "general"}
        <General bind:config logPath={payload.logPath} {onchange} />
      {:else if section === "shortcuts"}
        <Shortcuts bind:config {problems} {onchange} />
      {:else if section === "setup"}
        <LinuxSetup />
      {:else if section === "privacy"}
        <Privacy bind:config accessibility={payload.accessibility} {onchange} />
      {:else if section === "licences"}
        {#await import("./ThirdPartyLicences.svelte") then { default: ThirdPartyLicences }}
          <ThirdPartyLicences back={() => (section = "about")} />
        {/await}
      {:else}
        <div class="page">
          <h1>About SuperEditr</h1>
          <p class="lede">Version {payload.version}. Fast, private, keyboard-first proofreading that works in any app, with the AI you choose.</p>
          <div class="group">
            <div class="row"><span>The rule-based checker is <button class="linklike" onclick={() => api.openUrl("https://writewithharper.com")}>Harper</button> (Apache-2.0).</span></div>
            <div class="row"><span>The secret-detection rules include patterns from <button class="linklike" onclick={() => api.openUrl("https://github.com/gitleaks/gitleaks")}>Gitleaks</button> (MIT).</span></div>
            <div class="row"><button class="btn" onclick={() => (section = "licences")}>Third-party licences</button></div>
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
  .recovery {
    color: var(--warn);
    background: var(--warn-bg);
    border-radius: 8px;
    max-width: 720px;
    margin-bottom: 16px;
    padding: 12px;
  }
  .recovery p {
    margin: 4px 0;
  }
  .recovery code {
    overflow-wrap: anywhere;
  }
  .recovery button {
    margin-top: 8px;
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
  /* Narrow, as when a tiling desktop puts Settings beside another window: less room for the sidebar
     and the margins, so the page keeps its width. */
  @media (max-width: 700px) {
    .shell {
      grid-template-columns: 168px minmax(0, 1fr);
    }
    main {
      padding: 0 18px 32px;
    }
  }
</style>
