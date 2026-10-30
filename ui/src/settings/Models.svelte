<script lang="ts">
  // One choice of AI. Choosing one opens its setup; it becomes the one in use only once it
  // passes a check (key saved, server answers), so a misconfigured AI never takes over reviews.
  import { onMount } from "svelte";
  import type { BuiltinAi, Candidate, Config, ProviderCheck, ProviderKind, ProviderProfile } from "../lib/types";
  import { watchBuiltinAi } from "./builtin";
  import { api } from "./ipc";
  import { earlyAccess, profileFor as pickProfile, setupAction } from "./profiles";

  interface Props {
    config: Config;
    keys: Record<string, boolean>;
    onchange: () => void;
    reload: () => Promise<void>;
    /** Scan as soon as the page opens (Get Started's "Find my AI accounts"); cleared once it starts. */
    scanOnOpen?: boolean;
    /** Shown until setup is finished: returns to Get Started. */
    backToSetup?: (() => void) | null;
  }
  let { config = $bindable(), keys, onchange, reload, scanOnOpen = $bindable(false), backToSetup = null }: Props = $props();
  /** The AI that became the one in use on this visit, for the way back to Get Started. */
  let setUp = $state("");
  /** Draws the eye to the scan row when Get Started sent the user here. */
  let highlight = $state(false);

  // Alphabetical, with the catch-all server last.
  const kinds: { kind: ProviderKind; name: string; keyUrl?: string; blurb: string }[] = [
    { kind: "anthropic", name: "Claude API", keyUrl: "https://console.anthropic.com/settings/keys", blurb: "Claude Haiku is fast and inexpensive. Needs an API key." },
    { kind: "claude_cli", name: "Claude subscription", blurb: "Uses your signed-in Claude Code. Slower to start." },
    { kind: "gemini", name: "Gemini API", keyUrl: "https://aistudio.google.com/apikey", blurb: "Google AI Studio keys include a free tier." },
    { kind: "ollama", name: "Ollama", blurb: "Private and free, runs on this Mac." },
    { kind: "open_ai", name: "OpenAI API", keyUrl: "https://platform.openai.com/api-keys", blurb: "GPT models. Needs an API key." },
    { kind: "open_ai_compatible", name: "Other server", blurb: "LM Studio, OpenRouter, Groq, or any OpenAI-compatible endpoint." },
  ];

  type Choice = ProviderKind | "builtin";
  let checks = $state<Record<string, ProviderCheck | "checking">>({});
  let keyDrafts = $state<Record<string, string>>({});
  let tests = $state<Record<string, { text: string; ok: boolean }>>({});
  let pulling = $state<{ status: string; pct: number } | null>(null);
  let pullModel = $state("gemma3:4b");
  let builtin = $state<BuiltinAi | null>(null);

  // One row per kind, with an account picker when several AIs of a kind are saved (see profiles.ts).
  let picked = $state<Partial<Record<ProviderKind, string>>>({});
  /** The exact saved AI in use; with two of a kind, the other one isn't. */
  const isActive = (id: string) => id === config.active_provider;
  const profilesOf = (kind: ProviderKind) => config.providers.filter((p) => p.kind === kind);
  const profileFor = (kind: ProviderKind) => pickProfile(config.providers, kind, picked[kind], config.active_provider);
  const accountLabel = (p: ProviderProfile) => p.base_url || (keys[p.id] ? `key saved (${p.id})` : `no key (${p.id})`);
  const activeChoice = $derived<Choice>(config.providers.find((p) => p.id === config.active_provider)?.kind ?? "builtin");
  /** The row whose setup is open: the one in use, until another is chosen. */
  let opened = $state<Choice | null>(null);
  const shown = $derived(opened ?? activeChoice);
  const needsKey = (kind: ProviderKind) => kind === "anthropic" || kind === "open_ai" || kind === "gemini";

  // Scanning this Mac: each account found shows on its AI's row, ready to use, unless it's
  // already saved there.
  let candidates = $state<Candidate[] | null>(null);
  let scanning = $state(false);
  let scanError = $state("");
  let adopting = $state<number | null>(null);
  let adoptErrors = $state<Record<number, string>>({});
  const found = (kind: ProviderKind) => (candidates ?? []).filter((c) => c.kind === kind && c.profileId === null);
  const foundCount = $derived(kinds.reduce((n, k) => n + found(k.kind).length, 0));

  async function scan() {
    scanning = true;
    scanError = "";
    adoptErrors = {};
    try {
      candidates = await api.scan();
    } catch (e) {
      scanError = String(e);
    }
    scanning = false;
  }

  /** Saves a found account and makes it the AI in use if it passes its check. */
  async function adopt(c: Candidate) {
    adopting = c.index;
    delete adoptErrors[c.index];
    try {
      const profile = await api.adopt(c.index);
      c.profileId = profile.id;
      picked[c.kind] = profile.id;
      opened = c.kind;
      await reload();
      const p = profileFor(c.kind);
      if (p) await recheck(p);
      if (config.active_provider === profile.id) setUp = profile.label;
    } catch (e) {
      adoptErrors[c.index] = String(e);
      await reload();
    }
    adopting = null;
  }

  function problemLabel(problem: string): string {
    if (problem.startsWith("Add an API key")) return "Needs key";
    if (problem.startsWith("Choose a model")) return "Choose a model";
    if (problem.startsWith("No models")) return "No models";
    if (problem.startsWith("Sign in")) return "Sign in needed";
    return "Can’t connect";
  }

  function status(p: ProviderProfile | null): { label: string; tone: string } | null {
    if (!p) return null;
    const c = checks[p.id];
    const inUse = config.active_provider === p.id;
    if (c === "checking") return { label: "Checking…", tone: "" };
    if (c && !c.ready) return { label: problemLabel(c.problem ?? ""), tone: "warn" };
    if (inUse) return { label: "In use", tone: "accent" };
    return c?.ready ? { label: "Ready", tone: "good" } : null;
  }

  async function recheck(p: ProviderProfile) {
    checks[p.id] = "checking";
    checks[p.id] = await api.checkProvider(p.id);
  }

  /** Makes `p` the AI in use if it works; otherwise shows what's missing and keeps the current one. */
  async function tryActivate(p: ProviderProfile) {
    checks[p.id] = "checking";
    try {
      checks[p.id] = await api.activateProvider(p.id);
      await reload();
      if (config.active_provider === p.id) setUp = p.label;
    } catch (e) {
      checks[p.id] = { ready: false, problem: String(e), models: [] };
    }
  }

  async function choose(choice: Choice) {
    opened = choice;
    if (choice === "builtin") {
      if (builtin?.available) {
        config.active_provider = null;
        onchange();
      }
      return;
    }
    let p = profileFor(choice);
    if (!p) {
      await api.addProvider(choice);
      await reload();
      p = profileFor(choice);
    }
    if (p) await tryActivate(p);
  }

  /** A radio click: the button only becomes checked once the choice takes effect (an AI that
   * fails its check leaves the current one checked). */
  const pick = (choice: Choice) => (e: MouseEvent) => {
    e.preventDefault();
    choose(choice);
  };

  /** Opens an AI's setup (adding it if needed) without making it the one in use. */
  async function openSetup(kind: ProviderKind) {
    opened = kind;
    let p = profileFor(kind);
    if (!p) {
      await api.addProvider(kind);
      await reload();
      p = profileFor(kind);
    }
    if (p) await recheck(p);
  }

  /** Returns reviews to the built-in AI (or the rule-based checker without one), keeping this AI
   * and its key for later. Works whether or not Apple Intelligence is available. */
  function stopUsing() {
    config.active_provider = null;
    setUp = "";
    opened = null;
    onchange();
  }

  /** After a setup field changes: try again to make it the AI in use. */
  async function retry(p: ProviderProfile) {
    await api.save(config);
    await tryActivate(p);
  }

  async function saveKey(p: ProviderProfile) {
    await api.setKey(p.id, keyDrafts[p.id] ?? "");
    keyDrafts[p.id] = "";
    await reload();
    await tryActivate(p);
  }

  async function remove(p: ProviderProfile) {
    await api.removeProvider(p.id);
    delete checks[p.id];
    opened = "builtin";
    await reload();
  }

  async function test(p: ProviderProfile) {
    tests[p.id] = { text: "Timing a review…", ok: true };
    try {
      const r = await api.test(p.id);
      tests[p.id] = { text: `✓ Answered in ${(r.totalMs / 1000).toFixed(1)} s: “${r.output}”`, ok: true };
    } catch (e) {
      tests[p.id] = { text: `⚠︎ ${e}`, ok: false };
    }
  }

  async function pull(p: ProviderProfile) {
    pulling = { status: "Starting download…", pct: 0 };
    try {
      await api.pull(p.base_url, pullModel);
      p.model = pullModel;
      pulling = null;
      await retry(p);
    } catch (e) {
      pulling = { status: `⚠︎ ${e}`, pct: 0 };
    }
  }

  onMount(() => {
    let stop: (() => void) | undefined;
    api
      .on<{ status: string; completed: number; total: number }>("settings:pull", (p) => {
        pulling = { status: p.status, pct: p.total ? Math.round((p.completed / p.total) * 100) : pulling?.pct ?? 0 };
      })
      .then((s) => (stop = s));
    const unwatch = watchBuiltinAi(api.builtinAi, (b) => (builtin = b));
    // Catch a key that stopped working or a server that's down before the next review does.
    for (const k of kinds) {
      const p = profileFor(k.kind);
      if (p) recheck(p);
    }
    // Sent from Get Started: let the page settle in view, then scan, with the row highlighted.
    let startScan: ReturnType<typeof setTimeout> | undefined;
    if (scanOnOpen) {
      scanOnOpen = false;
      highlight = true;
      startScan = setTimeout(() => {
        scan();
        setTimeout(() => (highlight = false), 1200);
      }, 300);
    }
    return () => {
      stop?.();
      unwatch();
      clearTimeout(startScan);
    };
  });
</script>

<div class="page">
  {#if backToSetup}
    <div class="back">
      <button class="linklike" onclick={backToSetup}>← Back to Get Started</button>
      {#if setUp}<span>{setUp} is set up. Go back to finish setup.</span>{/if}
    </div>
  {/if}
  <h1>Models</h1>
  <p class="lede">
    SuperEditr uses one AI at a time. Apple Intelligence runs on this Mac and suits most people; a cloud AI gives stronger
    rewrites. If the one you choose can’t be reached, Apple Intelligence takes over. Your text goes only to the AI you choose,
    and keys stay in your Keychain.
  </p>

  <div class="group scan" class:highlight>
    <div class="row">
      <div>
        <div class="label">Find AI accounts on this Mac</div>
        <span class="hint">
          {#if scanning}Looking…
          {:else if scanError}<span class="warn-text">{scanError}</span>
          {:else if candidates && foundCount}Found {foundCount === 1 ? "an account" : `${foundCount} accounts`}, marked below.
          {:else if candidates}Nothing new found. Set up an AI below, or install <button class="linklike" onclick={() => api.openUrl("https://ollama.com/download")}>Ollama</button> for free local models.
          {:else}Looks for API keys in your shell profile and Codex settings, a signed-in Claude Code, and Ollama or LM Studio running locally. Nothing is sent anywhere.{/if}
        </span>
      </div>
      <button class="btn small" onclick={scan} disabled={scanning}>{scanning ? "Scanning…" : candidates ? "Scan again" : "Scan"}</button>
    </div>
  </div>


  <div class="group-title">Choose an AI</div>
  <div class="group choices">
    {#if builtin?.supported}
      <div class="choice" class:open={shown === "builtin"}>
        <label class="pick">
          <input type="radio" name="ai" checked={activeChoice === "builtin"} disabled={!builtin.available} onclick={pick("builtin")} />
          <span>
            <span class="label">Apple Intelligence</span>
            <span class="pill">Built in</span>
            <span class="pill good">Recommended</span>
            {#if !builtin.available}<span class="pill warn">Off</span>
            {:else if activeChoice === "builtin"}<span class="pill accent">In use</span>
            {:else}<span class="pill backup">Backup</span>{/if}
            <span class="hint">
              {#if !builtin.available}{builtin.message}
              {:else if activeChoice === "builtin"}Runs entirely on this Mac: private, works offline, fast, and free. It’s also the backup whenever another AI can’t be reached.
              {:else}Standing by on this Mac: it answers whenever {kinds.find((k) => k.kind === activeChoice)?.name ?? "your AI"} can’t be reached, including when you’re offline.{/if}
            </span>
          </span>
        </label>
        {#if !builtin.available}<button class="btn small" onclick={() => api.openAppleIntelligence()}>Turn on…</button>{/if}
      </div>
    {/if}
    {#each kinds as k (k.kind)}
      {@const p = profileFor(k.kind)}
      {@const st = status(p)}
      {@const c = p ? checks[p.id] : undefined}
      <div class="choice" class:open={shown === k.kind}>
        <label class="pick">
          <input type="radio" name="ai" checked={activeChoice === k.kind} onclick={pick(k.kind)} />
          <span>
            <span class="label">{k.name}</span>
            {#if earlyAccess(k.kind, p)}<span class="pill early" title="Included in early access">Early access</span>{/if}
            {#if st}<span class="pill {st.tone}">{st.label}</span>{/if}
            <span class="hint">{k.blurb}</span>
          </span>
        </label>
        {#if shown !== k.kind && !(found(k.kind).length && !p)}<button class="btn small" onclick={() => openSetup(k.kind)}>{p ? "Settings" : "Set up"}</button>{/if}
      </div>
      {#each found(k.kind) as fc (fc.index)}
        <div class="found">
          <span>
            <span class="label">Found on this Mac {#if fc.keyHint}<span class="pill">{fc.keyHint}</span>{/if}</span>
            <span class="hint">{fc.source}{#if fc.models.length} · {fc.models.length} model{fc.models.length === 1 ? "" : "s"}{/if}</span>
            {#if fc.note}<span class="hint note">{fc.note}</span>{/if}
            {#if adoptErrors[fc.index]}<span class="alert">⚠︎ {adoptErrors[fc.index]}</span>{/if}
          </span>
          <button class="btn small primary" disabled={adopting !== null || !fc.ready} onclick={() => adopt(fc)}>
            {adopting === fc.index ? "Setting up…" : "Use"}
          </button>
        </div>
      {/each}
      {#if shown === k.kind && p}
        <div class="setup">
          {#if c && c !== "checking" && !c.ready}
            <p class="problem">⚠︎ {c.problem}{!isActive(p.id) ? `. Reviews keep using ${activeChoice === "builtin" ? "Apple Intelligence" : (kinds.find((x) => x.kind === activeChoice)?.name ?? "the current AI")}.` : ""}</p>
          {/if}
          {#if profilesOf(k.kind).length > 1}
            <div class="row">
              <span class="label">Account</span>
              <select value={p.id} onchange={(e) => { picked[k.kind] = e.currentTarget.value; const q = profileFor(k.kind); if (q) recheck(q); }}>
                {#each profilesOf(k.kind) as q (q.id)}
                  <option value={q.id}>{accountLabel(q)}{q.id === config.active_provider ? " · in use" : ""}</option>
                {/each}
              </select>
            </div>
          {/if}
          {#if needsKey(k.kind)}
            <div class="row">
              <span class="label">API key</span>
              <span class="inline">
                <input type="password" placeholder={keys[p.id] ? "•••••••• (saved)" : "Paste key"} bind:value={keyDrafts[p.id]} />
                <button class="btn small" onclick={() => saveKey(p)} disabled={!keyDrafts[p.id]}>Save</button>
                {#if k.keyUrl}<button class="linklike" onclick={() => api.openUrl(k.keyUrl!)}>Get a key</button>{/if}
              </span>
            </div>
          {/if}
          {#if k.kind === "ollama" || k.kind === "open_ai_compatible" || k.kind === "claude_cli"}
            <div class="row">
              <span class="label">{k.kind === "claude_cli" ? "Claude CLI path" : "Server URL"}</span>
              <input type="text" class="wide" bind:value={p.base_url} onchange={() => retry(p)} spellcheck="false" />
            </div>
          {/if}
          <div class="row">
            <span class="label">Model</span>
            <span class="inline">
              {#if c && c !== "checking" && c.models.length}
                <select bind:value={p.model} onchange={() => retry(p)}>
                  {#if !c.models.includes(p.model)}<option value={p.model}>{p.model}</option>{/if}
                  {#each c.models as m (m)}<option value={m}>{m}</option>{/each}
                </select>
              {:else}
                <input type="text" bind:value={p.model} onchange={() => retry(p)} spellcheck="false" />
              {/if}
            </span>
          </div>
          {#if k.kind === "ollama"}
            <div class="row">
              <span>
                <span class="label">Download a model</span>
                <span class="hint">Small models (3–4B) answer fastest. Try gemma3:4b, qwen3:4b, or llama3.2:3b.</span>
              </span>
              <span class="inline">
                <input type="text" bind:value={pullModel} spellcheck="false" />
                <button class="btn small" onclick={() => pull(p)} disabled={!!pulling && pulling.pct < 100 && !pulling.status.startsWith("⚠")}>Download</button>
              </span>
            </div>
            {#if pulling}
              <div class="row progress"><span>{pulling.status}</span><progress max="100" value={pulling.pct}></progress></div>
            {/if}
          {/if}
          <div class="row actions-row">
            <span class="inline">
              <button class="btn small" onclick={() => (setupAction(p, config.active_provider) === "check" ? recheck(p) : tryActivate(p))}>
                {setupAction(p, config.active_provider) === "check" ? "Check again" : "Use this AI"}
              </button>
              <button class="btn small" onclick={() => test(p)} disabled={!c || c === "checking" || !c.ready}>Speed check</button>
            </span>
            <span class="inline">
              {#if isActive(p.id)}
                <button
                  class="btn small"
                  title={builtin?.available ? "Apple Intelligence reviews instead. This AI and its key stay saved." : "The rule-based checker reviews instead. This AI and its key stay saved."}
                  onclick={stopUsing}>Stop using</button>
              {/if}
              <button class="btn small danger" onclick={() => remove(p)}>Remove</button>
            </span>
          </div>
          {#if tests[p.id]}<div class="row test" class:failed={!tests[p.id].ok}>{tests[p.id].text}</div>{/if}
        </div>
      {/if}
    {/each}
  </div>

</div>

<style>
  .back {
    display: flex;
    gap: 10px;
    align-items: baseline;
    margin-bottom: 14px;
    font-size: 13px;
    color: var(--text-2);
  }
  .scan {
    transition: box-shadow 300ms ease;
  }
  .scan.highlight {
    box-shadow: 0 0 0 3px var(--accent-soft), 0 0 0 1px var(--accent);
  }
  .found {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    padding: 8px 14px 10px 42px;
    background: var(--accent-soft);
  }
  .found .label {
    font-size: 12.5px;
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .note {
    color: var(--text-2);
  }
  .warn-text {
    color: var(--warn);
  }
  /* What stops a found account from working ("Sign in to Claude Code…"): it must stand out on
     the found row's tint, so it gets the warning colors rather than a grey hint. */
  .alert {
    display: inline-block;
    margin-top: 6px;
    padding: 4px 9px;
    border-radius: 7px;
    background: var(--warn-bg);
    color: var(--warn);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--warn) 35%, transparent);
    font-size: 12px;
    font-weight: 600;
  }
  .choice {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    padding: 12px 14px;
    border-top: 1px solid var(--border);
  }
  .choice:first-child {
    border-top: none;
  }
  .choice.open {
    background: var(--bg-sunken);
  }
  .setup {
    background: var(--bg-sunken);
    padding: 0 0 6px 28px;
  }
  .setup .row {
    border-top: none;
    padding: 6px 14px;
  }
  .problem {
    margin: 0 14px 6px;
    padding: 6px 10px;
    border-radius: 8px;
    background: var(--warn-bg);
    color: var(--warn);
    font-size: 12.5px;
  }
  .test.failed {
    color: var(--warn);
  }
  .pick {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    cursor: pointer;
  }
  .pick input {
    margin-top: 3px;
    accent-color: var(--accent);
  }
  .pick .label {
    margin-right: 6px;
  }
  .hint {
    display: block;
    color: var(--text-3);
    font-size: 12px;
    margin-top: 3px;
  }
  .actions,
  .inline {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  /* Fixed widths, so a provider's longest model name (OpenAI lists very long ones) can't stretch
     the model picker past the panel. */
  .inline input[type="password"],
  .inline input[type="text"],
  .inline select {
    width: 220px;
    max-width: 100%;
    text-overflow: ellipsis;
  }
  .wide {
    width: 320px;
  }
  .test {
    font-size: 12px;
    color: var(--text-2);
    justify-content: flex-start;
  }
  .progress progress {
    width: 200px;
    accent-color: var(--accent);
  }
</style>
