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
  /** The log's path with the home folder as ~, or on Windows the local app data folder as
   * %LOCALAPPDATA%, as people write them. */
  const shownLogPath = $derived(
    (logPath ?? "").replace(/^\/(Users|home)\/[^/]+\//, "~/").replace(/^[A-Za-z]:\\Users\\[^\\]+\\AppData\\Local\\/i, "%LOCALAPPDATA%\\"),
  );
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
        <label class="label" for="auto-popup">Suggest as soon as I select text</label>
        <div class="hint" id="auto-popup-hint">Shows suggestions automatically when you select text you’re editing. Typing or clicking elsewhere hides them. Turn off to use the shortcut only.</div>
      </div>
      <input id="auto-popup" type="checkbox" class="switch" aria-describedby="auto-popup-hint" bind:checked={config.auto_popup} onchange={onchange} />
    </div>
    <div class="row">
      <div>
        <label class="label" for="auto-min">Minimum selection</label>
        <div class="hint" id="auto-min-hint">Shorter selections are ignored for automatic suggestions.</div>
      </div>
      <span><input id="auto-min" type="number" min="1" max="500" aria-describedby="auto-min-unit auto-min-hint" bind:value={config.auto_min_chars} onchange={onchange} style="width: 70px" /> <span id="auto-min-unit">characters</span></span>
    </div>
    <div class="row">
      <div>
        <label class="label" for="auto-max">Maximum selection</label>
        <div class="hint" id="auto-max-hint">Longer selections, such as a whole document, are ignored for automatic suggestions. The shortcut still reviews any selection.</div>
      </div>
      <span><input id="auto-max" type="number" min="500" max="100000" aria-describedby="auto-max-unit auto-max-hint" bind:value={config.auto_max_chars} onchange={onchange} style="width: 70px" /> <span id="auto-max-unit">characters</span></span>
    </div>
    {/if}
    <div class="row">
      <div>
        <label class="label" for="keep-formatting">Keep formatting when applying fixes</label>
        <div class="hint" id="keep-formatting-hint">Proofread fixes change only the words that need fixing, so code, bold and links stay as they are. Undo takes one step per fix.</div>
      </div>
      <input id="keep-formatting" type="checkbox" class="switch" aria-describedby="keep-formatting-hint" bind:checked={config.keep_formatting} onchange={onchange} />
    </div>
    <div class="row">
      <label class="label" for="show-timing">Show response times</label>
      <input id="show-timing" type="checkbox" class="switch" bind:checked={config.show_timing} onchange={onchange} />
    </div>
  </div>

  <div class="group-title">Startup</div>
  <div class="group">
    <div class="row">
      <div>
        <label class="label" for="launch-at-login">Open at login</label>
        <div class="hint" id="launch-at-login-hint">Starts SuperEditr in {w.tray} when you log in, so it’s running again after you restart {w.yourDevice}.</div>
      </div>
      <input id="launch-at-login" type="checkbox" class="switch" aria-describedby="launch-at-login-hint" checked={launchAtLogin} onchange={(e) => setLaunchAtLogin(e.currentTarget.checked)} />
    </div>
    {#if launchError}
      <div class="row warn">{launchError}</div>
    {/if}
  </div>

  <div class="group-title">Troubleshooting</div>
  <div class="group">
    <div class="row">
      <div>
        <label class="label" for="diagnostic-log">Keep a diagnostic log</label>
        <div class="hint" id="diagnostic-log-hint">Records which app each selection came from, with timings and lengths but never your text, in {shownLogPath}. Turn on to diagnose a problem in a particular app. Turning it off deletes the log.</div>
      </div>
      <input id="diagnostic-log" type="checkbox" class="switch" aria-describedby="diagnostic-log-hint" bind:checked={config.diagnostic_log} onchange={onchange} />
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
