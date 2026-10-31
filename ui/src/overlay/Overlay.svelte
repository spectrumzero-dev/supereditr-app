<script lang="ts">
  import { onMount, tick } from "svelte";
  import { slide } from "svelte/transition";
  import DiffText from "../lib/DiffText.svelte";
  import { applyView, changeLabel, changes, sourceLabel, summarize } from "../lib/diff";
  import type { EngineEvent, SessionPayload, StatusPayload } from "../lib/types";
  import { ipc } from "./ipc";
  import { canUse, emptyReview, offerToKeep, reduce, type Review } from "./review";

  let session = $state<SessionPayload | null>(null);
  let presetId = $state("proofread");
  let review = $state<Review>(emptyReview());
  let notice = $state("");
  let rejected = $state(new Set<number>());
  let focusedId = $state<number | null>(null);
  /** The panel takes the review keys: a review the user opened, or "use keyboard". */
  let keyboard = $state(true);
  let steer = $state("");
  let steering = $state(false);
  /** A brief confirmation ("Copied"), shown in a row that slides out below the footer. */
  let flash = $state("");
  let flashTimer: ReturnType<typeof setTimeout> | undefined;
  /** Words added to the word list from this panel, so the offer doesn't repeat. */
  let added = $state(new Set<string>());
  let card: HTMLElement | undefined = $state();
  let steerInput: HTMLInputElement | undefined = $state();

  const view = $derived(review.view);
  const phase = $derived(review.phase);
  const source = $derived(review.source);
  const timing = $derived(review.timing);
  const error = $derived(review.error);
  const changedTokens = $derived(review.changedTokens);
  const keptTerms = $derived(review.keptTerms);
  const backupFor = $derived(review.backupFor);
  const usable = $derived(canUse(review));
  const list = $derived(changes(view));
  const result = $derived(view ? applyView(view, rejected) : session?.text ?? "");
  const focusedChange = $derived(list.find((c) => c.id === focusedId) ?? null);
  const unchanged = $derived(phase === "done" && list.length === 0);
  const busy = $derived(phase === "local" || phase === "streaming" || phase === "idle");
  const keepOffer = $derived(offerToKeep(review, rejected, focusedId, added));

  /** Key tips for the first few reviews, until the keys are familiar. */
  const TIP_REVIEWS = 3;
  let showTips = $state(false);
  function countTip(): boolean {
    try {
      const seen = Number(localStorage.getItem("tipsSeen") ?? 0);
      if (seen >= TIP_REVIEWS) return false;
      localStorage.setItem("tipsSeen", String(seen + 1));
      return true;
    } catch {
      return false;
    }
  }

  function reset(next: SessionPayload) {
    session = next;
    presetId = next.presetId;
    review = emptyReview();
    notice = "";
    rejected = new Set();
    focusedId = null;
    keyboard = !next.auto;
    showTips = keyboard && countTip();
    steer = "";
    steering = false;
    clearTimeout(flashTimer);
    flash = "";
    added = new Set();
  }

  function onEngine(id: number, eventPreset: string, event: EngineEvent) {
    if (!session || id !== session.id || eventPreset !== presetId) return;
    const wasStreaming = review.phase === "streaming";
    review = reduce(review, event);
    // Changes that would drop the author's own terms start out rejected (␣ accepts one).
    if (event.type === "done") rejected = new Set(event.kept);
    else if (event.type === "failed" && wasStreaming) rejected = new Set();
  }

  function runPreset(id: string, instruction?: string) {
    if (!session) return;
    presetId = id;
    review = emptyReview();
    rejected = new Set();
    focusedId = null;
    ipc.runPreset(session.id, id, instruction?.trim() || null);
  }

  function cycle(step: number) {
    if (list.length === 0) return;
    const index = list.findIndex((c) => c.id === focusedId);
    const next = index === -1 ? (step > 0 ? 0 : list.length - 1) : (index + step + list.length) % list.length;
    focusedId = list[next].id;
  }

  function toggle(id: number) {
    const next = new Set(rejected);
    next.has(id) ? next.delete(id) : next.add(id);
    rejected = next;
  }

  async function apply() {
    // Only a finished result can be applied: never a revision that is still streaming or failed.
    if (!session || !usable) return;
    if (!session.canApply) return copy();
    const text = result;
    const id = session.id;
    session = null;
    try {
      await ipc.apply(id, text);
    } catch {
      /* the backend shows the reason */
    }
  }

  async function alwaysKeep(word: string) {
    try {
      const isNew = await ipc.addWord(word);
      added = new Set([...added, word]);
      showFlash(isNew === false ? `“${word}” is already in your words` : `Added “${word}” to your words`);
    } catch {
      // The offer stays, so the author can try again.
      showFlash(`Couldn’t save “${word}”. Try again.`);
    }
  }

  function showFlash(message: string, ms = 2000) {
    clearTimeout(flashTimer);
    flash = message;
    flashTimer = setTimeout(() => (flash = ""), ms);
  }

  async function copy() {
    if (!usable) return;
    await ipc.copy(result);
    showFlash("Copied", 1200);
  }

  function close() {
    ipc.dismiss(session?.id ?? null);
    session = null;
    notice = "";
  }

  function useKeyboard() {
    keyboard = true;
    ipc.focus();
  }

  function startSteering() {
    steering = true;
    // The panel takes keyboard focus only for typing the instruction.
    ipc.typing(true).then(tick).then(() => steerInput?.focus());
  }

  function stopSteering() {
    steering = false;
    steerInput?.blur();
    ipc.typing(false);
  }

  /** A key typed into the panel, or one the keyboard tap took from the app (`forwarded`). */
  type PanelKey = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "preventDefault">;

  function onKeydown(e: PanelKey, forwarded = false) {
    if (notice && !session) {
      if (e.key === "Escape" || e.key === "Enter") close();
      return;
    }
    if (!session) return;
    const mod = e.metaKey || e.ctrlKey;
    if (steering) {
      if (e.key === "Escape") {
        e.preventDefault();
        steer = "";
        stopSteering();
      } else if (e.key === "Enter") {
        e.preventDefault();
        stopSteering();
        runPreset(presetId, steer);
      } else if (forwarded && !mod) {
        // Typed after / but before the panel had focus.
        if (e.key === "Backspace") steer = steer.slice(0, -1);
        else if (e.key.length === 1) steer += e.key;
      }
      return;
    }
    if (mod && /^[1-9]$/.test(e.key)) {
      const preset = session.presets[Number(e.key) - 1];
      if (preset) runPreset(preset.id);
      e.preventDefault();
      return;
    }
    switch (e.key) {
      case "Enter":
        e.preventDefault();
        apply();
        break;
      case "Escape":
        e.preventDefault();
        close();
        break;
      case "Tab":
        e.preventDefault();
        cycle(e.shiftKey ? -1 : 1);
        break;
      case "ArrowRight":
      case "ArrowDown":
        e.preventDefault();
        cycle(1);
        break;
      case "ArrowLeft":
      case "ArrowUp":
        e.preventDefault();
        cycle(-1);
        break;
      case " ":
      case "Backspace":
      case "Delete":
        if (focusedId != null) {
          e.preventDefault();
          toggle(focusedId);
        }
        break;
      case "/":
        e.preventDefault();
        startSteering();
        break;
      default:
        if (mod && e.key === "c") {
          e.preventDefault();
          copy();
        } else if ((mod && e.key === "r") || (!mod && e.key === "r")) {
          e.preventDefault();
          runPreset(presetId);
        } else if (!mod && e.key === "a" && keepOffer) {
          e.preventDefault();
          alwaysKeep(keepOffer);
        } else if (mod && e.key === ",") {
          e.preventDefault();
          ipc.openSettings();
        } else if (!mod && /^[1-9]$/.test(e.key)) {
          const preset = session.presets[Number(e.key) - 1];
          if (preset) runPreset(preset.id);
        }
    }
  }

  // The window is reset to a starting height for every review or notice, but ResizeObserver only
  // reports changes: a repeat review the same height as the last would stay cut off. Report the
  // height once the new content has rendered, after the reset has been applied.
  function reportHeight() {
    tick().then(() =>
      requestAnimationFrame(() => {
        if (card) ipc.resize(Math.ceil(card.getBoundingClientRect().height) + 54);
      }),
    );
  }

  onMount(() => {
    const unlisten = ipc.listen({
      session: (payload) => {
        reset(payload);
        reportHeight();
      },
      engine: (id, preset, event) => onEngine(id, preset, event),
      status: (payload: StatusPayload) => {
        session = null;
        notice = payload.message;
        reportHeight();
      },
      focus: () => (keyboard = true),
      command: (command) => {
        if (command === "apply") apply();
      },
      key: (key) => onKeydown({ ...key, preventDefault: () => {} }, true),
    });
    const observer = new ResizeObserver(reportHeight);
    if (card) observer.observe(card);
    return () => {
      unlisten();
      observer.disconnect();
    };
  });
</script>

<svelte:window onkeydown={onKeydown} />

<main>
  <section class="card" bind:this={card} class:hidden={!session && !notice} aria-live="polite">
    {#if notice && !session}
      <div class="notice">
        <div class="notice-icon" aria-hidden="true">!</div>
        <p>{notice}</p>
        <div class="notice-actions">
          <button class="ghost" onclick={() => ipc.openSettings()}>Settings</button>
          <button class="primary" onclick={close}>OK</button>
        </div>
      </div>
    {:else if session}
      <header>
        <nav class="presets" aria-label="Styles">
          {#each session.presets.slice(0, 7) as preset, i (preset.id)}
            <button
              class="chip"
              class:active={preset.id === presetId}
              title={`${preset.name} (${i + 1})`}
              onclick={() => runPreset(preset.id)}
            >
              {preset.name}{#if keyboard}<span class="num">{i + 1}</span>{/if}
            </button>
          {/each}
        </nav>
      </header>

      <div class="body">
        {#if view}
          <DiffText {view} {rejected} focused={focusedId} streaming={phase === "streaming"} ontoggle={toggle} />
        {:else}
          <div class="diff-placeholder">{session.text}</div>
        {/if}
        {#if unchanged}
          <p class="all-good"><span aria-hidden="true">✓</span> Looks good — nothing to change.</p>
        {/if}
        {#if error}
          <p class="error">{error}</p>
        {/if}
        {#if backupFor}
          <p class="error">{backupFor}. Apple Intelligence answered instead.</p>
        {/if}
        {#if keptTerms.length || keepOffer}
          <p class="kept">
            {#if keptTerms.length}Kept your wording: {keptTerms.slice(0, 3).join(", ")}{keptTerms.length > 3 ? "…" : ""}{/if}
            {#if keptTerms.length && keepOffer}·{/if}
            {#if keepOffer}
              <button class="link" onclick={() => alwaysKeep(keepOffer)}>Always keep “{keepOffer}”</button>{#if keyboard}&nbsp;<kbd>A</kbd>{/if}
            {/if}
          </p>
        {/if}
        {#if changedTokens.length}
          <p class="error">Check before applying: this version changes {changedTokens.slice(0, 3).join(", ")}{changedTokens.length > 3 ? "…" : ""}</p>
        {/if}
        {#if focusedChange}
          <p class="note">
            <strong>{changeLabel(focusedChange)}</strong>
            {focusedChange.note ?? (rejected.has(focusedChange.id) ? "Keeping your original." : `“${focusedChange.old.trim()}” → “${focusedChange.new.trim()}”`)}
          </p>
        {/if}
      </div>

      {#if steering}
        <div class="steer">
          <span aria-hidden="true">✦</span>
          <input
            bind:this={steerInput}
            bind:value={steer}
            placeholder="Tell SuperEditr what to change, e.g. “make it warmer and shorter”"
            spellcheck="false"
          />
          <kbd>↩</kbd>
        </div>
      {/if}

      {#if showTips && keyboard && phase === "done" && list.length}
        <p class="tips">Tip: <kbd>⌥↩</kbd> {session.canApply ? "replaces your text with" : "copies"} everything you see · <kbd>tab</kbd> steps through changes · <kbd>␣</kbd> switches a change between yours and the suggestion · <kbd>esc</kbd> closes</p>
      {/if}
      <footer>
        <div class="status" title={[summarize(view), sourceLabel(source, view), timing].filter(Boolean).join(" · ")}>
          {#if busy}
            <span class="dot pulsing" aria-hidden="true"></span>
            {phase === "streaming" && source ? `${source}…` : "Checking…"}
          {:else if phase === "failed"}
            <span class="dot bad" aria-hidden="true"></span> Couldn’t finish
          {:else}
            <span class="dot good" aria-hidden="true"></span>
            {list.length ? `${list.length} change${list.length === 1 ? "" : "s"}` : "No changes"}
          {/if}
          <!-- The backup note above explains a stand-in, so the footer keeps the name short; the
               name may shorten with an ellipsis, the time never does. -->
          {#if phase === "done" && source}<span class="muted source">· {sourceLabel(source.replace(" (backup)", ""), view)}</span>{/if}
          {#if session.showTiming && timing}<span class="muted time">· {timing}</span>{/if}
        </div>
        <!-- ⌥↩ works in every panel, so it is the one replace key shown; ↩ also works in a review
             the user opened. Nothing to replace, nothing to show. -->
        <div class="keys">
          {#if keyboard}
            {#if list.length > 1}<span><kbd>tab</kbd> next</span>{/if}
            {#if focusedId != null}<span><kbd>␣</kbd> {rejected.has(focusedId) ? "restore" : "keep original"}</span>{/if}
            <span><kbd>/</kbd> instruct</span>
          {/if}
          {#if !unchanged}<span><kbd>⌥↩</kbd> {session.canApply ? "replace" : "copy fix"}</span>{/if}
          {#if keyboard}
            <span><kbd>esc</kbd></span>
          {:else}
            <button class="link" onclick={useKeyboard}>use keyboard</button>
          {/if}
        </div>
      </footer>
      {#if flash}
        <div class="flash" role="status" transition:slide={{ duration: 180 }}>{flash}</div>
      {/if}
    {/if}
  </section>
</main>

<style>
  :global(body.overlay-body) {
    background: transparent;
    overflow: hidden;
    user-select: none;
    -webkit-user-select: none;
    cursor: default;
  }
  /* Room for the whole card shadow (--shadow-card reaches 14px up, 24px sideways, 34px down):
     the window clips anything outside it, which shows as a hard edge. Kept in step with
     overlay.rs PAD_* and the +54 height report. */
  main {
    padding: 16px 26px 38px;
  }
  .card {
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow-card);
    overflow: hidden;
    animation: rise 140ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .card.hidden {
    visibility: hidden;
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(-4px) scale(0.99);
    }
  }
  header {
    padding: 8px 10px 0;
  }
  .presets {
    display: flex;
    gap: 4px;
    overflow-x: auto;
    scrollbar-width: none;
    mask-image: linear-gradient(90deg, #000 calc(100% - 28px), transparent);
    -webkit-mask-image: linear-gradient(90deg, #000 calc(100% - 28px), transparent);
  }
  .chip {
    all: unset;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 3px 8px;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-2);
    cursor: pointer;
    white-space: nowrap;
  }
  .chip:hover {
    background: var(--bg-hover);
    color: var(--text);
  }
  .chip.active {
    background: var(--accent);
    color: var(--accent-text);
  }
  .chip .num {
    font-size: 10px;
    opacity: 0.55;
  }
  .body {
    padding: 10px 14px 12px;
    max-height: 420px;
    overflow-y: auto;
    user-select: text;
    -webkit-user-select: text;
  }
  .diff-placeholder {
    white-space: pre-wrap;
    line-height: 1.6;
    font-size: 14px;
    color: var(--text-3);
  }
  .all-good {
    margin: 8px 0 0;
    color: var(--ins);
    font-weight: 500;
  }
  .tips {
    margin: 0;
    padding: 8px 18px;
    font-size: 12px;
    color: var(--text-2);
    background: var(--accent-soft);
    border-top: 1px solid var(--border);
  }
  .kept {
    margin: 0 18px 10px;
    font-size: 12px;
    color: var(--text-2);
  }
  .error {
    margin: 8px 0 0;
    padding: 6px 10px;
    border-radius: 8px;
    background: var(--warn-bg);
    color: var(--warn);
  }
  .note {
    margin: 8px 0 0;
    font-size: 12px;
    color: var(--text-2);
  }
  .note strong {
    color: var(--text);
    margin-right: 4px;
  }
  .steer {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 10px 10px;
    padding: 6px 10px;
    border-radius: 10px;
    border: 1px solid var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .steer input {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    font: inherit;
    font-size: 13px;
    color: var(--text);
  }
  footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
    padding: 7px 12px;
    border-top: 1px solid var(--border);
    background: var(--bg-sunken);
    font-size: 11.5px;
    color: var(--text-2);
  }
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
  }
  .status .source {
    min-width: 0;
    text-overflow: ellipsis;
  }
  .status .time {
    flex: none;
  }
  .muted {
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .flash {
    padding: 7px 12px;
    border-top: 1px solid var(--border);
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 12px;
    font-weight: 600;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-3);
    flex: none;
  }
  .dot.good {
    background: var(--ins);
  }
  .dot.bad {
    background: var(--del);
  }
  .dot.pulsing {
    background: var(--accent);
    animation: pulse 1s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }
  .keys {
    display: flex;
    gap: 10px;
    white-space: nowrap;
    flex: none;
  }
  .link {
    all: unset;
    color: var(--accent);
    cursor: pointer;
  }
  .notice {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 12px;
    align-items: center;
    padding: 14px 16px;
  }
  .notice p {
    margin: 0;
    font-size: 13px;
    line-height: 1.45;
  }
  .notice-icon {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 700;
  }
  .notice-actions {
    grid-column: 2;
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .primary,
  .ghost {
    border-radius: 7px;
    padding: 4px 12px;
    border: 1px solid var(--border-strong);
    background: var(--bg);
    cursor: pointer;
  }
  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-text);
  }
</style>
