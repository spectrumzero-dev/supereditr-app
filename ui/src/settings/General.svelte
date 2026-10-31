<script lang="ts">
  import { onMount } from "svelte";
  import type { Config } from "../lib/types";
  import { api } from "./ipc";
  import { isMac, words } from "../lib/platform";

  interface Props {
    config: Config;
    /** Where capture.log is written. */
    logPath: string | null;
    onchange: () => void;
  }
  let { config = $bindable(), logPath, onchange }: Props = $props();
  const w = words();
  /** The log's path with the home folder as ~, as people write it. */
  const shownLogPath = $derived((logPath ?? "").replace(/^\/(Users|home)\/[^/]+\//, "~/"));
  let launchAtLogin = $state(true);
  let launchError = $state("");

  onMount(() => {
    api.launchAtLogin().then((on) => (launchAtLogin = on));
  });

  async function setLaunchAtLogin(enabled: boolean) {
    launchError = "";
    try {
      launchAtLogin = await api.setLaunchAtLogin(enabled);
    } catch (e) {
      launchError = String(e);
      launchAtLogin = await api.launchAtLogin();
    }
  }
</script>

<div class="page">
  <h1>General</h1>
  <p class="lede">When suggestions appear, how fixes are applied, and starting at login.</p>

  <div class="group-title">Suggestions</div>
  <div class="group">
    <!-- Automatic suggestions need a system-wide view of selections, which only macOS gives. -->
    {#if isMac()}
    <div class="row">
      <div>
        <div class="label">Suggest as soon as I select text</div>
        <div class="hint">Shows suggestions automatically when you select text you’re editing. Typing or clicking elsewhere hides them. Turn off to use the shortcut only.</div>
      </div>
      <input type="checkbox" class="switch" bind:checked={config.auto_popup} onchange={onchange} />
    </div>
    <div class="row">
      <div>
        <div class="label">Minimum selection</div>
        <div class="hint">Shorter selections are ignored for automatic suggestions.</div>
      </div>
      <span><input type="number" min="1" max="500" bind:value={config.auto_min_chars} onchange={onchange} style="width: 70px" /> characters</span>
    </div>
    {/if}
    <div class="row">
      <div>
        <div class="label">Keep formatting when applying fixes</div>
        <div class="hint">Proofread fixes change only the words that need fixing, so code, bold and links stay as they are. Undo takes one step per fix.</div>
      </div>
      <input type="checkbox" class="switch" bind:checked={config.keep_formatting} onchange={onchange} />
    </div>
    <div class="row">
      <div class="label">Show response times</div>
      <input type="checkbox" class="switch" bind:checked={config.show_timing} onchange={onchange} />
    </div>
  </div>

  <div class="group-title">Startup</div>
  <div class="group">
    <div class="row">
      <div>
        <div class="label">Open at login</div>
        <div class="hint">Starts SuperEditr in {w.tray} when you log in, so it’s running again after you restart {isMac() ? "your Mac" : "your computer"}.</div>
      </div>
      <input type="checkbox" class="switch" aria-label="Open at login" checked={launchAtLogin} onchange={(e) => setLaunchAtLogin(e.currentTarget.checked)} />
    </div>
    {#if launchError}
      <div class="row warn">{launchError}</div>
    {/if}
  </div>

  <div class="group-title">Troubleshooting</div>
  <div class="group">
    <div class="row">
      <div>
        <div class="label">Keep a diagnostic log</div>
        <div class="hint">Records which app each selection came from, with timings and lengths but never your text, in {shownLogPath}. Turn on to diagnose a problem in a particular app. Turning it off deletes the log.</div>
      </div>
      <input type="checkbox" class="switch" bind:checked={config.diagnostic_log} onchange={onchange} />
    </div>
  </div>
</div>

<style>
  .hint {
    color: var(--text-3);
    font-size: 12px;
    margin-top: 2px;
    max-width: 480px;
    line-height: 1.4;
  }
  .label {
    font-weight: 500;
  }
  .warn {
    color: var(--warn);
    background: var(--warn-bg);
  }
</style>
