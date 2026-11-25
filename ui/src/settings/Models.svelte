<script lang="ts">
  // One choice of AI. Choosing one opens its setup; it becomes the one in use only once it
  // passes a check (key saved, server answers), so a misconfigured AI never takes over reviews.
  import { onMount } from "svelte";
  import type { BuiltinAi, Candidate, CheckCode, Config, ProviderCheck, ProviderKind, ProviderProfile } from "../lib/types";
  import { watchBuiltinAi } from "./builtin";
  import { api } from "./ipc";
  import { earlyAccess, profileFor as pickProfile, setupAction } from "./profiles";
  import { isWindows, words } from "../lib/platform";
  import ClaudeSignIn from "./ClaudeSignIn.svelte";
  import { problemText } from "./signin";

  interface Props {
    config: Config;
    keys: Record<string, boolean>;
    /** Saves the config now, based on the settings the app last gave Settings. False when it
     * couldn't be saved. */
    saveNow: () => Promise<boolean>;
    reload: () => Promise<void>;
    onWriteFailure: (message: string, error: unknown) => void;
    /** Scan as soon as the page opens (Get Started's "Find my AI accounts"); cleared once it starts. */
    scanOnOpen?: boolean;
    /** Shown until setup is finished: returns to Get Started. */
    backToSetup?: (() => void) | null;
  }
  let { config = $bindable(), keys, saveNow, reload, onWriteFailure, scanOnOpen = $bindable(false), backToSetup = null }: Props = $props();
  /** The AI that became the one in use on this visit, for the way back to Get Started. */
  let setUp = $state("");
  /** Draws the eye to the scan row when Get Started sent the user here. */
  let highlight = $state(false);

  // Alphabetical, with the catch-all server last.
  const kinds: { kind: ProviderKind; name: string; keyUrl?: string; blurb: string }[] = [
    { kind: "anthropic", name: "Claude API", keyUrl: "https://console.anthropic.com/settings/keys", blurb: "Claude Sonnet gives careful, reliable edits. Needs an API key." },
    { kind: "claude_cli", name: "Claude subscription", blurb: "Uses your signed-in Claude Code. Slower to start." },
    { kind: "gemini", name: "Gemini API", keyUrl: "https://aistudio.google.com/apikey", blurb: "Google AI Studio keys include a free tier." },
    { kind: "ollama", name: "Ollama", blurb: `Private and free, runs on ${words().device}.` },
    { kind: "open_ai", name: "OpenAI API", keyUrl: "https://platform.openai.com/api-keys", blurb: "GPT models. Needs an API key." },
    { kind: "open_ai_compatible", name: "Other server", blurb: "LM Studio, OpenRouter, Groq, or any OpenAI-compatible endpoint." },
  ];

  type Choice = ProviderKind | "builtin";
  let checks = $state<Record<string, ProviderCheck | "checking">>({});
  let keyDrafts = $state<Record<string, string>>({});
  let tests = $state<Record<string, { text: string; ok: boolean }>>({});
  let choosing = $state(false);
  let choiceError = $state("");
  let pullRequest = $state<string | null>(null);
  let pulling = $state<{ profileId: string; status: string; pct: number } | null>(null);
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
  /** A found account added but not ready, with its profile and check, when the check has a code
   * Settings can act on (Claude Code not signed in). */
  let adoptChecks = $state<Record<number, { profileId: string; check: ProviderCheck }>>({});
  /** Profiles signed in to Claude from here on this visit, whose warning no longer shows. */
  let signedIn = $state<Record<string, boolean>>({});
  const found = (kind: ProviderKind) => (candidates ?? []).filter((c) => c.kind === kind && c.profileId === null);
  const foundCount = $derived(kinds.reduce((n, k) => n + found(k.kind).length, 0));

  async function scan() {
    if (scanning || choosing) return;
    scanning = true;
    scanError = "";
    adoptErrors = {};
    adoptChecks = {};
    try {
      candidates = await api.scan();
    } catch (e) {
      scanError = String(e);
    }
    scanning = false;
  }

  async function makeChoice(action: () => Promise<void>) {
    if (choosing) return;
    choosing = true;
    choiceError = "";
    try {
      await action();
    } catch (e) {
      choiceError = String(e);
    } finally {
      choosing = false;
    }
  }

  function offerRetryForChangedSettings(error: unknown): boolean {
    const message = error instanceof Error ? error.message : String(error);
    if (!message.startsWith("The settings file changed on disk.")) return false;
    onWriteFailure("Couldn’t save the AI settings.", error);
    return true;
  }

  /** Saves a found account and makes it the AI in use if it passes its check. */
  function adopt(c: Candidate) {
    return makeChoice(async () => {
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
        if (offerRetryForChangedSettings(e)) return;
        adoptErrors[c.index] = String(e);
        await reload();
        const added = profilesOf(c.kind).find((q) => q.base_url === c.baseUrl);
        const check = added ? await api.checkProvider(added.id).catch(() => null) : null;
        if (added && check?.code === "claude_sign_in") adoptChecks[c.index] = { profileId: added.id, check };
      } finally {
        adopting = null;
      }
    });
  }

  const problemLabels: Record<CheckCode, string> = {
    claude_sign_in: "Sign in needed",
    needs_key: "Needs key",
    choose_model: "Choose a model",
    no_models: "No models",
  };

  function problemLabel(check: ProviderCheck): string {
    return check.code ? problemLabels[check.code] : "Can’t connect";
  }

  function status(p: ProviderProfile | null): { label: string; tone: string } | null {
    if (!p) return null;
    const c = checks[p.id];
    const inUse = config.active_provider === p.id;
    if (c === "checking") return { label: "Checking…", tone: "" };
    if (c && !c.ready) return { label: problemLabel(c), tone: "warn" };
    if (inUse) return { label: "In use", tone: "accent" };
    return c?.ready ? { label: "Ready", tone: "good" } : null;
  }

  /** Counts each profile's changes and checks, so a check that finishes after a change (a new key,
   * model or address) doesn't show its result for the old settings. */
  const generation: Record<string, number> = {};
  const nextGeneration = (id: string) => (generation[id] = (generation[id] ?? 0) + 1);

  /** Check again, saying what it found: a check that passes looks just like before it ran. */
  async function checkAgain(p: ProviderProfile) {
    const mine = nextGeneration(p.id);
    delete tests[p.id];
    checks[p.id] = "checking";
    let c: ProviderCheck;
    try {
      c = await api.checkProvider(p.id);
    } catch (e) {
      c = { ready: false, problem: String(e), models: [] };
    }
    if (generation[p.id] !== mine) return;
    checks[p.id] = c;
    const at = new Date().toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
    tests[p.id] = c.ready
      ? { text: `✓ Works. Checked at ${at}.`, ok: true }
      : { text: `Still not working at ${at}. The problem is shown above.`, ok: false };
  }

  async function recheck(p: ProviderProfile) {
    const mine = nextGeneration(p.id);
    checks[p.id] = "checking";
    const check = await api.checkProvider(p.id).catch((e) => ({ ready: false, problem: String(e), models: [] }));
    if (generation[p.id] === mine) checks[p.id] = check;
  }

  /** Makes `p` the AI in use if it works; otherwise shows what's missing and keeps the current one. */
  async function tryActivate(p: ProviderProfile) {
    // A result from before a change (key, model, address) no longer describes this AI, and a
    // Check again still running is for the old settings.
    nextGeneration(p.id);
    delete tests[p.id];
    checks[p.id] = "checking";
    try {
      checks[p.id] = await api.activateProvider(p.id);
      await reload();
      if (config.active_provider === p.id) setUp = p.label;
    } catch (e) {
      if (offerRetryForChangedSettings(e)) {
        delete checks[p.id];
        return;
      }
      // The check behind the refusal, for a problem Settings words itself.
      const again = await api.checkProvider(p.id).catch(() => null);
      checks[p.id] = again && !again.ready ? again : { ready: false, problem: String(e), code: null, models: [] };
    }
  }

  async function findOrAdd(kind: ProviderKind): Promise<ProviderProfile | null> {
    const existing = profileFor(kind);
    if (existing) return existing;
    try {
      await api.addProvider(kind);
    } catch (error) {
      onWriteFailure("Couldn’t add the AI.", error);
      return null;
    }
    await reload();
    return profileFor(kind);
  }

  function choose(choice: Choice) {
    return makeChoice(async () => {
      if (choice === "builtin") {
        opened = choice;
        if (builtin?.available) await useBuiltin();
        return;
      }
      const p = await findOrAdd(choice);
      if (p) {
        opened = choice;
        await tryActivate(p);
      }
    });
  }

  /** A radio click: the button only becomes checked once the choice takes effect (an AI that
   * fails its check leaves the current one checked). */
  const pick = (choice: Choice) => (e: MouseEvent) => {
    e.preventDefault();
    choose(choice);
  };

  /** Opens an AI's setup (adding it if needed) without making it the one in use. */
  function openSetup(kind: ProviderKind) {
    return makeChoice(async () => {
      const p = await findOrAdd(kind);
      if (p) {
        opened = kind;
        await recheck(p);
      }
    });
  }

  /** Returns reviews to the built-in AI (or the rule-based checker without one), keeping this AI
   * and its key for later. Works whether or not Apple Intelligence is available. */
  async function useBuiltin() {
    try {
      await api.useBuiltinProvider();
    } catch (error) {
      onWriteFailure("Couldn’t switch to the built-in AI.", error);
      return;
    }
    await reload();
    setUp = "";
    opened = "builtin";
  }

  /** After a setup field changes: try again to make it the AI in use. */
  function retry(p: ProviderProfile) {
    return makeChoice(async () => {
      // Unsaved, the check would run against the address or model the app still has.
      if (!(await saveNow())) {
        checks[p.id] = { ready: false, problem: "Couldn’t save this change. Select Retry in Settings.", code: null, models: [] };
        return;
      }
      await tryActivate(p);
    });
  }

  function saveKey(p: ProviderProfile) {
    return makeChoice(async () => {
      try {
        await api.setKey(p.id, keyDrafts[p.id] ?? "");
      } catch (error) {
        onWriteFailure("Couldn’t save the API key.", error);
        return;
      }
      keyDrafts[p.id] = "";
      await reload();
      await tryActivate(p);
    });
  }

  function remove(p: ProviderProfile) {
    return makeChoice(async () => {
      try {
        await api.removeProvider(p.id);
        nextGeneration(p.id);
        delete checks[p.id];
        opened = "builtin";
      } catch (error) {
        onWriteFailure("Couldn’t remove the AI.", error);
      } finally {
        await reload();
      }
    });
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

  function pull(p: ProviderProfile) {
    const { id, base_url, label } = p;
    const model = pullModel.trim();
    return makeChoice(async () => {
      const requestId = crypto.getRandomValues(new Uint32Array(4)).join("-");
      pullRequest = requestId;
      pulling = { profileId: id, status: "Starting download…", pct: 0 };
      nextGeneration(id);
      try {
        checks[id] = await api.pull(requestId, id, base_url, model);
        pulling = null;
        await reload();
        if (config.active_provider === id) setUp = label;
      } catch (e) {
        pulling = offerRetryForChangedSettings(e) ? null : { profileId: id, status: `⚠︎ ${e}`, pct: 0 };
      } finally {
        pullRequest = null;
      }
    });
  }

  onMount(() => {
    let stop: (() => void) | undefined;
    let mounted = true;
    api
      .on("settings:pull", ({ requestId, progress }) => {
        if (requestId !== pullRequest || !pulling) return;
        const { status, completed, total } = progress;
        pulling = { ...pulling, status, pct: total ? Math.round((completed / total) * 100) : pulling.pct };
      })
      .then((s) => { if (mounted) stop = s; else s(); });
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
      mounted = false;
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
  {#if builtin?.supported}
    <p class="lede">
      SuperEditr uses one AI at a time. Apple Intelligence runs on this Mac and suits most people; a cloud AI gives stronger
      rewrites. If the one you choose can’t be reached, Apple Intelligence takes over. Your text goes only to the AI you choose,
      and keys stay in your Keychain.
    </p>
  {:else}
    <p class="lede">
      SuperEditr uses one AI at a time. Ollama runs on {words().device} and keeps your text there; a cloud AI gives stronger
      rewrites. Without one, or when it can’t be reached, a built-in checker fixes English spelling and grammar. Your text goes
      only to the AI you choose, and keys stay in {words().keyStore}.
    </p>
  {/if}

  <div class="group scan" class:highlight>
    <div class="row">
      <div>
        <div class="label" id="scan-label">Find AI accounts on {words().device}</div>
        <span class="hint" id="scan-hint">
          {#if scanning}Looking…
          {:else if scanError}<span class="warn-text">{scanError}</span>
          {:else if candidates && foundCount}Found {foundCount === 1 ? "an account" : `${foundCount} accounts`}, marked below.
          {:else if candidates}Nothing new found. Set up an AI below, or install <button class="linklike" onclick={() => api.openUrl("https://ollama.com/download")}>Ollama</button> for free local models.
          {:else}Looks for API keys in your {isWindows() ? "environment variables" : "shell profile"} and Codex settings, a signed-in Claude Code, and Ollama or LM Studio running locally. Nothing is sent anywhere.{/if}
        </span>
      </div>
      <button class="btn small" aria-describedby="scan-label scan-hint" onclick={scan} disabled={scanning || choosing}>{scanning ? "Scanning…" : candidates ? "Scan again" : "Scan"}</button>
    </div>
  </div>


  <div class="group-title">Choose an AI</div>
  {#if choosing && !pullRequest}<p role="status">Checking…</p>{/if}
  {#if choiceError}<p class="problem" role="alert">⚠︎ {choiceError}</p>{/if}
  <div class="group choices">
    {#if builtin?.supported}
      <div class="choice" class:open={shown === "builtin"}>
        <label class="pick">
          <input type="radio" name="ai" aria-labelledby="builtin-ai-label" aria-describedby="builtin-ai-status builtin-ai-hint" checked={activeChoice === "builtin"} disabled={!builtin.available || choosing} onclick={pick("builtin")} />
          <span>
            <span class="label" id="builtin-ai-label">Apple Intelligence</span>
            <span id="builtin-ai-status">
              <span class="pill">Built in</span>
              <span class="pill good">Recommended</span>
              {#if !builtin.available}<span class="pill warn">Off</span>
              {:else if activeChoice === "builtin"}<span class="pill accent">In use</span>
              {:else}<span class="pill backup">Backup</span>{/if}
            </span>
            <span class="hint" id="builtin-ai-hint">
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
          <input type="radio" name="ai" aria-labelledby={`${k.kind}-label`} aria-describedby={`${k.kind}-status ${k.kind}-hint`} checked={activeChoice === k.kind} disabled={choosing} onclick={pick(k.kind)} />
          <span>
            <span class="label" id={`${k.kind}-label`}>{k.name}</span>
            <span id={`${k.kind}-status`}>
              {#if earlyAccess(k.kind, p)}<span class="pill early" title="Included in early access">Early access</span>{/if}
              {#if st}<span class="pill {st.tone}">{st.label}</span>{/if}
            </span>
            <span class="hint" id={`${k.kind}-hint`}>{k.blurb}</span>
          </span>
        </label>
        {#if shown !== k.kind && !(found(k.kind).length && !p)}<button class="btn small" disabled={choosing} onclick={() => openSetup(k.kind)}>{p ? "Settings" : "Set up"}</button>{/if}
      </div>
      {#each found(k.kind) as fc (fc.index)}
        {@const signInHere = adoptChecks[fc.index]?.check.code === "claude_sign_in" && isWindows()}
        <div class="found">
          <span>
            <span class="label">Found on {words().device} {#if fc.keyHint}<span class="pill">{fc.keyHint}</span>{/if}</span>
            <span class="hint">{fc.source}{#if fc.models.length} · {fc.models.length} model{fc.models.length === 1 ? "" : "s"}{/if}</span>
            {#if fc.note}<span class="hint note">{fc.note}</span>{/if}
            {#if adoptChecks[fc.index]}
              {@const ac = adoptChecks[fc.index]}
              {#if !signedIn[ac.profileId]}<span class="alert">⚠︎ {problemText(ac.check)}</span>{/if}
              <!-- Signing in puts the account in use, so there's no Use to press afterwards. -->
              {#if signInHere}
                <ClaudeSignIn profileId={ac.profileId} disabled={choosing} onsignedin={() => { signedIn[ac.profileId] = true; delete adoptChecks[fc.index]; adopt(fc); }} />
              {/if}
            {:else if adoptErrors[fc.index]}<span class="alert">⚠︎ {adoptErrors[fc.index]}</span>{/if}
          </span>
          <!-- Sign in is the only next step while it's pending; after a refusal fixed elsewhere, it's Try
               again. An account that can't be used yet (Ollama not running) has its note instead. -->
          {#if !signInHere && fc.ready}
            <button class="btn small primary" disabled={choosing || scanning} onclick={() => adopt(fc)}>
              {adopting === fc.index ? "Setting up…" : adoptErrors[fc.index] ? "Try again" : "Use"}
            </button>
          {/if}
        </div>
      {/each}
      {#if shown === k.kind && p}
        <div class="setup">
          {#if c && c !== "checking" && !c.ready}
            <p class="problem">⚠︎ {problemText(c)}{!isActive(p.id) ? `. Reviews keep using ${activeChoice === "builtin" ? (builtin?.available ? "Apple Intelligence" : "the built-in checker") : (kinds.find((x) => x.kind === activeChoice)?.name ?? "the current AI")}.` : ""}</p>
          {/if}
          {#if c && c !== "checking" && c.code === "claude_sign_in" && isWindows()}
            <div class="signin-row">
              <ClaudeSignIn profileId={p.id} disabled={choosing} onsignedin={() => { signedIn[p.id] = true; makeChoice(() => tryActivate(p)); }} />
            </div>
          {/if}
          {#if profilesOf(k.kind).length > 1}
            <div class="row">
              <label class="label" for={`${k.kind}-account`}>Account</label>
              <select id={`${k.kind}-account`} value={p.id} disabled={choosing} onchange={(e) => { picked[k.kind] = e.currentTarget.value; const q = profileFor(k.kind); if (q) recheck(q); }}>
                {#each profilesOf(k.kind) as q (q.id)}
                  <option value={q.id}>{accountLabel(q)}{q.id === config.active_provider ? " · in use" : ""}</option>
                {/each}
              </select>
            </div>
          {/if}
          {#if needsKey(k.kind)}
            <div class="row">
              <label class="label" for={`${k.kind}-api-key`}>API key</label>
              <span class="inline">
                <input id={`${k.kind}-api-key`} type="password" placeholder={keys[p.id] ? "•••••••• (saved)" : "Paste key"} bind:value={keyDrafts[p.id]} disabled={choosing} />
                <button class="btn small" onclick={() => saveKey(p)} disabled={!keyDrafts[p.id] || choosing}>Save</button>
                {#if k.keyUrl}<button class="linklike" onclick={() => api.openUrl(k.keyUrl!)}>Get a key</button>{/if}
              </span>
            </div>
          {/if}
          {#if k.kind === "ollama" || k.kind === "open_ai_compatible" || k.kind === "claude_cli"}
            <div class="row">
              <label class="label" for={`${k.kind}-address`}>{k.kind === "claude_cli" ? "Claude CLI path" : "Server URL"}</label>
              <input id={`${k.kind}-address`} type="text" class="wide" title={p.base_url} bind:value={p.base_url} disabled={choosing} onchange={() => retry(p)} spellcheck="false" />
            </div>
          {/if}
          <div class="row">
            <label class="label" for={`${k.kind}-model`}>Model</label>
            <span class="inline">
              {#if c && c !== "checking" && c.models.length}
                <select id={`${k.kind}-model`} bind:value={p.model} disabled={choosing} onchange={() => retry(p)}>
                  {#if !c.models.includes(p.model)}<option value={p.model}>{p.model}</option>{/if}
                  {#each c.models as m (m)}<option value={m}>{m}</option>{/each}
                </select>
              {:else}
                <input id={`${k.kind}-model`} type="text" bind:value={p.model} disabled={choosing} onchange={() => retry(p)} spellcheck="false" />
              {/if}
            </span>
          </div>
          {#if k.kind === "ollama"}
            <div class="row">
              <span>
                <label class="label" for="ollama-download-model">Download a model</label>
                <span class="hint" id="ollama-download-hint">Small models (3–4B) answer fastest. Try gemma3:4b, qwen3:4b, or llama3.2:3b.</span>
              </span>
              <span class="inline">
                <input id="ollama-download-model" type="text" aria-describedby="ollama-download-hint" bind:value={pullModel} disabled={choosing} spellcheck="false" />
                <button class="btn small" onclick={() => pull(p)} disabled={choosing || !pullModel.trim()}>Download</button>
              </span>
            </div>
            {#if pulling?.profileId === p.id}
              <div class="row progress" role="status"><span>{pulling.status}</span><progress aria-label="Model download" max="100" value={pulling.pct}></progress></div>
            {/if}
          {/if}
          <div class="row actions-row">
            <span class="inline">
              <button class="btn small" disabled={c === "checking" || choosing} onclick={() => (setupAction(p, config.active_provider) === "check" ? checkAgain(p) : makeChoice(() => tryActivate(p)))}>
                {c === "checking" ? "Checking…" : setupAction(p, config.active_provider) === "check" ? "Check again" : "Use this AI"}
              </button>
              <button class="btn small" onclick={() => test(p)} disabled={!c || c === "checking" || !c.ready}>Speed check</button>
            </span>
            <span class="inline">
              {#if isActive(p.id)}
                <button
                  class="btn small"
                  title={builtin?.available ? "Apple Intelligence reviews instead. This AI and its key stay saved." : "The rule-based checker reviews instead. This AI and its key stay saved."}
                  disabled={choosing}
                  onclick={() => makeChoice(useBuiltin)}>Stop using</button>
              {/if}
              <button class="btn small danger" disabled={choosing} onclick={() => remove(p)}>Remove</button>
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
    /* flow-root keeps a first child's top margin inside the tint instead of leaving a gap above it. */
    display: flow-root;
    background: var(--bg-sunken);
    padding: 0 0 6px 28px;
  }
  .signin-row {
    margin: 0 14px 6px;
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
  /* The web view's own focus ring is a square box; follow the circle instead. */
  .pick input:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
    border-radius: 50%;
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
