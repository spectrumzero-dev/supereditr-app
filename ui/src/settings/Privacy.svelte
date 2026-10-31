<script lang="ts">
  import type { AppInfo, Config } from "../lib/types";
  import { excludedRows, withApps } from "./excluded";
  import { api } from "./ipc";

  interface Props {
    config: Config;
    accessibility: boolean;
    onchange: () => void;
  }
  let { config = $bindable(), accessibility, onchange }: Props = $props();
  let newApp = $state("");
  /** What each excluded bundle ID's lookup found: the app, or null if it isn't installed. */
  let known = $state<Record<string, AppInfo | null>>({});
  /** Bundle IDs already looked up or being looked up. */
  const asked = new Set<string>();
  let picking = $state(false);
  let skipped = $state<string[]>([]);
  const rows = $derived(excludedRows(config.excluded_apps, known));

  $effect(() => {
    const ids = config.excluded_apps.filter((id) => !asked.has(id));
    if (!ids.length) return;
    ids.forEach((id) => asked.add(id));
    api
      .appInfo(ids)
      .catch(() => ({}) as Record<string, AppInfo>)
      .then((found) => {
        known = { ...known, ...Object.fromEntries(ids.map((id) => [id, found?.[id] ?? null])) };
      });
  });

  function add(ids: string[]) {
    const next = withApps(config.excluded_apps, ids);
    if (next.length !== config.excluded_apps.length) {
      config.excluded_apps = next;
      onchange();
    }
  }

  async function pickApps() {
    picking = true;
    try {
      const picked = await api.pickApps();
      picked.apps.forEach((app) => asked.add(app.bundleId));
      known = { ...known, ...Object.fromEntries(picked.apps.map((app) => [app.bundleId, app])) };
      add(picked.apps.map((app) => app.bundleId));
      skipped = picked.skipped;
    } finally {
      picking = false;
    }
  }

  function remove(id: string) {
    config.excluded_apps = config.excluded_apps.filter((a) => a !== id);
    onchange();
  }
</script>

<div class="page">
  <h1>Privacy</h1>
  <p class="lede">Your text goes only to the provider you pick, directly from this Mac. SuperEditr collects no analytics, and its only server counts update checks.</p>

  <div class="group-title">Permissions</div>
  <div class="group">
    <div class="row">
      <div>
        <div class="label">Accessibility {#if accessibility}<span class="pill good">Granted</span>{:else}<span class="pill bad">Needed</span>{/if}</div>
        <div class="hint">Lets SuperEditr read the text you select and replace it when you accept a suggestion. Password fields are never read.</div>
      </div>
      {#if !accessibility}
        <button class="btn primary" onclick={() => api.guideAccessibility()}>Grant access</button>
      {/if}
    </div>
  </div>

  <div class="group-title">Apps where SuperEditr never reads text</div>
  <div class="group">
    <div class="row">
      <button class="btn small" onclick={pickApps} disabled={picking}>Add app…</button>
      <form class="add-id" onsubmit={(e) => { e.preventDefault(); add([newApp]); newApp = ""; }}>
        <input type="text" placeholder="Or a bundle ID, e.g. com.apple.Notes" bind:value={newApp} spellcheck="false" />
        <button class="btn small" type="submit" disabled={!newApp.trim()}>Add</button>
      </form>
    </div>
    {#if skipped.length}
      <div class="row">
        <div class="hint">Couldn’t add {skipped.join(", ")}: SuperEditr recognizes apps by bundle ID, and {skipped.length === 1 ? "this one has" : "these have"} none.</div>
      </div>
    {/if}
    {#each rows as { id, app } (id)}
      <div class="row">
        <div class="app">
          {#if app?.icon}<img src={app.icon} alt="" />{:else}<span class="no-icon"></span>{/if}
          {#if app}
            <span class="label">{app.name}</span>
          {:else}
            <code>{id}</code><span class="muted">Not installed</span>
          {/if}
        </div>
        <button class="btn small" onclick={() => remove(id)}>Remove</button>
      </div>
    {/each}
  </div>
</div>

<style>
  code {
    font-family: var(--mono);
    font-size: 12px;
  }
  .add-id {
    display: flex;
    flex: 1;
    gap: 8px;
  }
  .add-id input {
    flex: 1;
  }
  .app {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .app img,
  .no-icon {
    flex: none;
    width: 24px;
    height: 24px;
  }
  .no-icon {
    box-sizing: border-box;
    border-radius: 6px;
    border: 1px dashed var(--border-strong);
  }
  .muted {
    font-size: 12px;
  }
</style>
