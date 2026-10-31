<script lang="ts">
  import { onMount } from "svelte";
  import type { BuiltinAi, Config, ProviderCheck } from "../lib/types";
  import { watchBuiltinAi } from "./builtin";
  import Playground from "./Playground.svelte";
  import Keycaps from "./Keycaps.svelte";
  import { keySymbols } from "../lib/diff";
  import { api } from "./ipc";

  interface Props {
    config: Config;
    accessibility: boolean;
    reload: () => Promise<void>;
    finish: () => void;
    /** Opens the Models page, to fix an AI that isn't working. */
    openModels: () => void;
    /** Opens the Models page and scans this Mac for AI accounts there. */
    findAccounts: () => void;
  }
  let { config, accessibility, reload, finish, openModels, findAccounts }: Props = $props();
  let polled = $state(false);
  let builtin = $state<BuiltinAi | null>(null);
  const trusted = $derived(accessibility || polled);
  /** Granted while this page was open: say so, since the user just came back from System Settings. */
  let justGranted = $state(false);
  const chosen = $derived(config.providers.find((p) => p.id === config.active_provider) ?? null);
  /** The chosen AI's check, so the badge is green only for one that works. */
  let chosenCheck = $state<ProviderCheck | null>(null);
  $effect(() => {
    const id = chosen?.id;
    chosenCheck = null;
    if (id) api.checkProvider(id).then((c) => id === chosen?.id && (chosenCheck = c));
  });

  /** Whether suggestions appear on their own after a selection, or only on the shortcut. */
  async function setAuto(on: boolean) {
    await api.save({ ...config, auto_popup: on });
    await reload();
  }

  function granted() {
    if (!trusted) justGranted = true;
    polled = true;
  }

  // The window fits this page: no empty space below, no scrolling. It follows the page as it
  // changes (access granted, accounts found) and goes back to the standard height on leaving.
  const STANDARD_HEIGHT = 760;
  const TITLE_BAR = 36; // App.svelte's drag strip above the page
  const BOTTOM = 40; // main's bottom padding
  let pageEl: HTMLDivElement | undefined = $state();
  function fit() {
    if (pageEl) api.fitHeight(Math.ceil(TITLE_BAR + pageEl.offsetHeight + BOTTOM));
  }

  onMount(() => {
    const observer = new ResizeObserver(fit);
    if (pageEl) observer.observe(pageEl);
    const unwatch = watchBuiltinAi(api.builtinAi, (b) => (builtin = b));
    // Granted in System Settings while this page is open; the guide also says so directly.
    const timer = setInterval(async () => {
      if (!trusted && (await api.accessibility(false))) granted();
    }, 1500);
    let stop: (() => void) | undefined;
    api.on("settings:accessibility-granted", granted).then((s) => (stop = s));
    return () => {
      observer.disconnect();
      api.fitHeight(STANDARD_HEIGHT);
      clearInterval(timer);
      unwatch();
      stop?.();
    };
  });
</script>

<div class="page welcome" bind:this={pageEl}>
  <div class="hero">
    <img src="/icon.png" alt="" width="48" height="48" />
    <div>
      <h1>Welcome to SuperEditr</h1>
      <p class="lede">Select text in any app, and fixes appear in under a second.</p>
    </div>
  </div>

  <!-- Before access is granted, nothing can be tried (SuperEditr can't see ⌘E E), so the page
       says so: allowing access comes first, and Try it now unlocks after. -->
  {#snippet tryCard()}
  <section class="try" class:locked={!trusted}>
    <div class="try-head">
      <h2>Try it now</h2>
      {#if chosen && chosenCheck && !chosenCheck.ready}
        <span class="ai off">{chosen.label} isn’t working: {chosenCheck.problem} · <button class="linklike" onclick={openModels}>Fix</button></span>
      {:else if chosen && chosenCheck}
        <span class="ai on">Using {chosen.label}{chosen.model ? ` · ${chosen.model}` : ""}</span>
      {:else if chosen}
        <span class="ai">Checking {chosen.label}…</span>
      {:else if builtin?.available}
        <span class="ai on" title="Private, works offline, fast, and free.">Apple Intelligence · on-device · recommended</span>
      {:else if builtin?.supported}
        <span class="ai off" title="Turn it on for private, offline fixes.">Apple Intelligence is off · <button class="linklike" onclick={() => api.openAppleIntelligence()}>Turn on</button></span>
      {:else if builtin}
        <span class="ai off">Rule-based spelling and grammar</span>
      {/if}
    </div>
    {#if trusted}
      <Playground {config} />
    {:else}
      <p class="locked-note">Allow access above, then try SuperEditr here and in every app.</p>
    {/if}
  </section>
  {/snippet}

  {#if trusted}{@render tryCard()}{/if}

  <ol class="steps">
    <li class:done={trusted} class:start={!trusted}>
      <div class="step-head">
        <span class="badge">{trusted ? "✓" : "1"}</span>
        <div>
          <div class="title" class:granted-title={justGranted}>
            {justGranted ? "Access granted: SuperEditr works in every app" : trusted ? "Use it in every app" : "Start here: allow access"}
          </div>
          {#if !trusted}
            <div class="hint">Allow SuperEditr to read and replace the text you select. It never reads password fields.</div>
          {/if}
        </div>
        {#if !trusted}
          <button class="btn primary" onclick={() => api.guideAccessibility()}>Allow…</button>
        {/if}
      </div>
      {#if trusted}
        <div class="inset tight"><Keycaps review={config.hotkeys.review} auto={config.auto_popup} /></div>
        <fieldset class="inset when">
          <legend>Show suggestions</legend>
          <label>
            <input type="radio" name="when" checked={config.auto_popup} onchange={() => setAuto(true)} />
            <span><span class="label">When I select text</span><span class="hint">They appear a moment after you select text you’re editing.</span></span>
          </label>
          <label>
            <input type="radio" name="when" checked={!config.auto_popup} onchange={() => setAuto(false)} />
            <span><span class="label">Only when I press {keySymbols(config.hotkeys.review)}</span><span class="hint">Nothing appears until you ask.</span></span>
          </label>
          <p class="hint">Change this anytime in Settings → General.</p>
        </fieldset>
      {:else}
        <p class="detail">System Settings opens at Accessibility: turn on SuperEditr’s switch (a card at the bottom helps if it isn’t listed). This page updates when it’s done.</p>
      {/if}
    </li>
    {#if !trusted}<li class="try-slot">{@render tryCard()}</li>{/if}

    <li class="optional">
      <div class="step-head">
        <span class="badge">2</span>
        <div>
          <div class="title">
            {builtin && !builtin.supported ? "Connect an AI for rewrites" : "Optional: connect a cloud AI for stronger rewrites"}
          </div>
          {#if builtin && !builtin.supported}
            <div class="hint">Without one, SuperEditr fixes English spelling and grammar offline. Rewrites and other languages need an AI.</div>
          {:else}
            <div class="hint">
              Claude, OpenAI, or Gemini do better with tone, long passages, and other languages. Your text then goes to that provider,
              and Apple Intelligence stays on as the offline backup.
            </div>
          {/if}
          {#if chosen}<div class="hint using">Using {chosen.label}{chosen.model ? ` · ${chosen.model}` : ""}</div>{/if}
        </div>
        <!-- One place finds and chooses AIs: Models. This opens it and starts the scan there. -->
        {#if chosen}
          <button class="btn" onclick={openModels}>Change in Models</button>
        {:else}
          <button class="btn" onclick={findAccounts}>Find my AI accounts →</button>
        {/if}
      </div>
    </li>
  </ol>

  <div class="finish">
    <button class="btn primary" onclick={finish}>Done</button>
  </div>
</div>

<style>
  .hero {
    display: flex;
    gap: 14px;
    align-items: center;
    margin-bottom: 16px;
  }
  .hero .lede {
    margin: 0;
  }
  .try {
    border: 1.5px solid var(--accent);
    border-radius: 14px;
    padding: 16px 18px 18px;
    background: var(--bg-raised);
    box-shadow: 0 0 0 4px var(--accent-soft);
    margin-bottom: 16px;
  }
  .try-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 12px;
    margin-bottom: 10px;
  }
  .try h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 650;
  }
  .ai {
    font-size: 12px;
    padding: 2px 9px;
    border-radius: 999px;
    white-space: nowrap;
  }
  .ai.on {
    background: var(--ins-bg);
    color: var(--ins);
  }
  .ai.off {
    background: var(--warn-bg);
    color: var(--warn);
  }
  .hint.using {
    margin-top: 6px;
    color: var(--text);
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 12px;
  }
  .steps > li.try-slot {
    padding: 0;
    border: none;
    background: none;
  }
  .steps > li.start {
    border: 1.5px solid var(--accent);
    box-shadow: 0 0 0 4px var(--accent-soft);
  }
  .try.locked {
    opacity: 0.6;
    border-color: var(--border);
    box-shadow: none;
    margin-bottom: 0;
  }
  .locked-note {
    margin: 0;
    color: var(--text-2);
  }
  .steps > li {
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 14px 16px;
    background: var(--bg-raised);
  }
  .step-head {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }
  .step-head > div {
    flex: 1;
  }
  .badge {
    width: 26px;
    height: 26px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 700;
    flex: none;
  }
  .done .badge {
    background: var(--ins-bg);
    color: var(--ins);
  }
  .optional .badge {
    background: var(--bg-sunken);
    color: var(--text-3);
  }
  .title {
    font-weight: 600;
    font-size: 14px;
    margin-top: 3px;
  }
  .hint {
    color: var(--text-2);
    font-size: 12.5px;
    margin-top: 3px;
    line-height: 1.5;
  }
  .detail {
    margin: 10px 0 0 38px;
    color: var(--text-2);
  }
  .inset {
    margin: 12px 0 0 38px;
  }
  .inset.tight {
    margin-top: 4px;
  }
  .when {
    border: none;
    padding: 0;
    display: grid;
    gap: 6px;
  }
  .when legend {
    font-weight: 600;
    font-size: 13px;
    padding: 0;
    margin-bottom: 6px;
  }
  .when label {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    cursor: pointer;
  }
  .when input {
    margin-top: 3px;
  }
  .when label > span {
    display: grid;
  }
  .when .label {
    font-size: 13px;
  }
  .when .hint {
    margin-top: 0;
  }
  .when > .hint {
    margin: 2px 0 0;
  }
  .granted-title {
    color: var(--ins);
  }
  .finish {
    display: flex;
    justify-content: flex-end;
    margin-top: 16px;
  }
</style>
