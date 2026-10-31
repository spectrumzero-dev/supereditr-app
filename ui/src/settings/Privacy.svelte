<script lang="ts">
  import type { Config } from "../lib/types";
  import { api } from "./ipc";

  interface Props {
    config: Config;
    accessibility: boolean;
    onchange: () => void;
  }
  let { config = $bindable(), accessibility, onchange }: Props = $props();
  let newApp = $state("");

  function addApp() {
    const id = newApp.trim();
    if (id && !config.excluded_apps.includes(id)) {
      config.excluded_apps = [...config.excluded_apps, id];
      onchange();
    }
    newApp = "";
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
      <input type="text" placeholder="Bundle ID, e.g. com.apple.Notes" bind:value={newApp} onkeydown={(e) => e.key === "Enter" && addApp()} spellcheck="false" />
      <button class="btn small" onclick={addApp} disabled={!newApp.trim()}>Add</button>
    </div>
    {#each config.excluded_apps as app (app)}
      <div class="row">
        <code>{app}</code>
        <button class="btn small" onclick={() => { config.excluded_apps = config.excluded_apps.filter((a) => a !== app); onchange(); }}>Remove</button>
      </div>
    {/each}
  </div>
</div>

<style>
  code {
    font-family: var(--mono);
    font-size: 12px;
  }
  .row input[type="text"] {
    flex: 1;
  }
</style>
