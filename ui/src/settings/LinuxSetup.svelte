<script lang="ts">
  import { onMount } from "svelte";
  import type { LinuxSetup } from "../lib/types";
  import { api } from "./ipc";

  interface Props {
    /** Inside Get Started: no page title. */
    embedded?: boolean;
  }
  let { embedded = false }: Props = $props();

  let report = $state<LinuxSetup | null>(null);
  /** Set up Omarchy lists its changes first; this is the confirming step. */
  let confirming = $state(false);
  let removing = $state(false);
  let busy = $state(false);
  let done = $state<string[]>([]);
  let error = $state<string | null>(null);
  const needed = $derived(report?.changes.filter((c) => c.needed) ?? []);
  const setUp = $derived(report !== null && needed.length === 0);

  async function load() {
    report = await api.linuxSetup();
  }

  async function run(action: () => Promise<string[]>) {
    busy = true;
    error = null;
    try {
      done = await action();
      if (done.length === 0) done = ["Nothing needed changing."];
    } catch (e) {
      error = String(e);
    }
    busy = false;
    confirming = false;
    removing = false;
    await load();
  }

  onMount(() => {
    load();
    // Apps open and close, and the shortcuts bind a moment after launch: keep the page current.
    const timer = setInterval(load, 4000);
    return () => clearInterval(timer);
  });
</script>

<div class:page={!embedded}>
  {#if !embedded}
    <h1>Desktop setup</h1>
    <p class="lede">What SuperEditr needs from your desktop, and how it’s doing.</p>
  {/if}

  {#if report}
    <div class="group">
      <div class="row head">
        <div>
          <div class="label">{report.desktop}</div>
          <div class="hint">
            {#if setUp}
              Set up: Chromium and Electron apps expose their text, and SuperEditr opens at login.
            {:else if report.omarchy}
              One button adds what SuperEditr needs. It shows each change first, and never touches Omarchy’s own files or your Hyprland config.
            {:else}
              Set up Omarchy changes only the files listed below, which other Arch desktops read too.
            {/if}
          </div>
        </div>
        {#if !setUp && !confirming}
          <button class="btn primary" disabled={busy} onclick={() => ((confirming = true), (removing = false), (done = []))}>Set up Omarchy…</button>
        {/if}
      </div>

      {#if confirming}
        <div class="row plan">
          <div>
            <div class="label">Set up Omarchy will make these changes:</div>
            <ul>
              {#each needed as change (change.path)}
                <li><code>{change.path}</code>: {change.what}</li>
              {/each}
            </ul>
            <div class="hint">Nothing under <code>~/.config/hypr/</code> or <code>/usr/share/omarchy</code> changes. Remove setup below, or <code>supereditr uninstall-omarchy</code>, takes out exactly these.</div>
          </div>
          <div class="actions">
            <button class="btn" disabled={busy} onclick={() => (confirming = false)}>Cancel</button>
            <button class="btn primary" disabled={busy} onclick={() => run(api.setUpOmarchy)}>Make these changes</button>
          </div>
        </div>
      {/if}

      {#if done.length}
        <div class="row result">
          <ul>{#each done as line, i (i)}<li>{line}</li>{/each}</ul>
        </div>
      {/if}
      {#if error}<div class="row warn">{error}</div>{/if}

      {#each report.checks as check (check.name)}
        <div class="row check">
          <span class="mark" class:ok={check.ok} aria-label={check.ok ? "OK" : "Needs attention"}>{check.ok ? "✓" : "!"}</span>
          <div>
            <div class="label">{check.name}</div>
            <div class="hint">{check.detail}</div>
          </div>
        </div>
      {/each}
    </div>

    {#if report.restart.length}
      <div class="group">
        <div class="row restart">
          <div>
            <div class="label">Restart these apps so SuperEditr can read them</div>
            <ul>
              {#each report.restart as app (app.name + app.reason)}
                <li><span class="app">{app.name}</span> <span class="hint">({app.reason})</span></li>
              {/each}
            </ul>
            <div class="hint">Until then, SuperEditr copies their selection, and suggestions go to the clipboard.</div>
          </div>
        </div>
      </div>
    {/if}

    {#if !embedded && report.changes.some((c) => !c.needed)}
      <div class="group">
        <div class="row">
          <div>
            <div class="label">Remove setup</div>
            <div class="hint">Takes out the flag lines SuperEditr added and stops it opening at login. Your settings and saved keys stay.</div>
          </div>
          {#if removing}
            <div class="actions">
              <button class="btn" disabled={busy} onclick={() => (removing = false)}>Cancel</button>
              <button class="btn danger" disabled={busy} onclick={() => run(api.removeOmarchySetup)}>Remove</button>
            </div>
          {:else}
            <button class="btn" disabled={busy} onclick={() => ((removing = true), (confirming = false), (done = []))}>Remove setup…</button>
          {/if}
        </div>
      </div>
    {/if}
  {:else}
    <p class="hint">Checking your desktop…</p>
  {/if}
</div>

<style>
  .label {
    font-weight: 500;
  }
  .hint {
    color: var(--text-3);
    font-size: 12px;
    margin-top: 2px;
    line-height: 1.4;
  }
  .head .hint {
    max-width: 440px;
  }
  /* Omarchy tiles Settings beside other windows: buttons go below the text when it's narrow. */
  .head,
  .plan {
    flex-wrap: wrap;
    gap: 10px;
  }
  .head > div,
  .plan > div:first-child {
    flex: 1 1 260px;
  }
  .check {
    justify-content: flex-start;
    gap: 12px;
    align-items: flex-start;
  }
  .mark {
    flex: none;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    font-size: 12px;
    font-weight: 700;
    color: var(--warn);
    background: var(--warn-bg);
  }
  .mark.ok {
    color: var(--ins);
    background: var(--ins-bg);
  }
  .plan,
  .restart {
    align-items: flex-start;
  }
  .plan,
  .result {
    background: var(--accent-soft);
  }
  ul {
    margin: 6px 0;
    padding-left: 18px;
    line-height: 1.5;
  }
  code {
    font-family: var(--mono);
    font-size: 12px;
  }
  .actions {
    display: flex;
    gap: 8px;
    flex: none;
  }
  .app {
    font-weight: 500;
  }
  .warn {
    color: var(--warn);
    background: var(--warn-bg);
  }
</style>
