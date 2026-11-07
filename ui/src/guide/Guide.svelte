<script lang="ts">
  // Floats beside System Settings → Accessibility. SuperEditr is almost always listed there already,
  // switched off: macOS lists any app that checks whether it has access, and SuperEditr checks at
  // launch. Nothing short of the access itself can confirm it's listed, so the card shows SuperEditr's
  // row with its switch turning on. The row is also the fallback: dragging it drags SuperEditr.app
  // itself (a native file drag started in Rust), which the Accessibility list accepts.
  import { invoke as tauriInvoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";

  const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  let error = $state("");

  /** Where the card sits relative to System Settings' window (`Side` in permission_guide.rs). */
  type Side = "right" | "left" | "below" | "inside";
  let side = $state<Side>("below");
  /** Where the list is, seen from the card, and the arrow's turn to point at it. */
  const toList: Record<Side, { words: string; turn: number }> = {
    below: { words: "above", turn: 0 },
    inside: { words: "below", turn: 180 },
    right: { words: "to the left", turn: -90 },
    left: { words: "to the right", turn: 90 },
  };
  const where = $derived(toList[side]);
  if (inTauri) listen<Side>("guide:side", (e) => (side = e.payload));

  // Imported up front, not on demand: the drag must start while the mouse button is still down.
  function invoke(cmd: string): Promise<unknown> {
    return inTauri ? tauriInvoke(cmd) : Promise.resolve();
  }

  function drag(e: MouseEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    invoke("drag_app").catch((err) => (error = String(err)));
  }
</script>

<div class="card">
  <button class="close" aria-label="Close" onclick={() => invoke("close_guide")}>×</button>
  <div class="head">
    <svg class="arrow" style:rotate="{where.turn}deg" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 20V5M5.5 11.5 12 5l6.5 6.5" /></svg>
    <p>Turn on <strong>SuperEditr</strong> in the list {where.words} to let it read and fix the text you select.</p>
  </div>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="tile" onmousedown={drag} title="Drag into the list {where.words}">
    <img src="/icon.png" alt="" width="28" height="28" draggable="false" />
    <span>SuperEditr</span>
    <span class="switch" aria-hidden="true"></span>
  </div>
  {#if error}<p class="error">{error}</p>{:else}<p class="hint">Not in the list? Drag this row into it.</p>{/if}
</div>

<style>
  :global(body.guide-body) {
    background: transparent;
    overflow: hidden;
    user-select: none;
    -webkit-user-select: none;
  }
  .card {
    position: relative;
    /* Room for the shadow to fade out inside the window: SHADOW in permission_guide.rs. */
    margin: 28px;
    padding: 14px 16px 10px;
    border-radius: 14px;
    background: var(--bg-raised);
    border: 1px solid var(--border-strong);
    /* Reaches at most 26px below the card, inside the 28px margin, so it never meets the edge. */
    box-shadow:
      0 6px 20px rgb(0 0 0 / 0.18),
      0 1px 3px rgb(0 0 0 / 0.12);
  }
  .head {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding-right: 18px;
  }
  .arrow {
    width: 22px;
    height: 22px;
    flex: none;
    fill: none;
    stroke: var(--accent);
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  p {
    margin: 0;
    font-size: 13.5px;
    line-height: 1.35;
  }
  .close {
    position: absolute;
    top: 6px;
    right: 8px;
    border: 0;
    background: none;
    color: var(--text-3);
    font-size: 18px;
    line-height: 1;
    cursor: pointer;
  }
  .tile {
    margin-top: 12px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 10px;
    border-radius: 9px;
    border: 1px solid var(--border-strong);
    background: var(--bg-sunken);
    font-size: 14px;
    font-weight: 500;
    cursor: grab;
  }
  .tile:active {
    cursor: grabbing;
  }
  .tile span:first-of-type {
    flex: 1;
  }
  /* A copy of the list's switch, turning on every few seconds. */
  .switch {
    position: relative;
    flex: none;
    width: 34px;
    height: 20px;
    border-radius: 10px;
    background: var(--border-strong);
    animation: switch-track 3.2s ease-in-out infinite;
  }
  .switch::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 2px rgb(0 0 0 / 0.3);
    animation: switch-knob 3.2s ease-in-out infinite;
  }
  @keyframes switch-track {
    0%,
    30% {
      background: var(--border-strong);
    }
    40%,
    90% {
      background: #0a84ff;
    }
    100% {
      background: var(--border-strong);
    }
  }
  @keyframes switch-knob {
    0%,
    30% {
      translate: 0;
    }
    40%,
    90% {
      translate: 14px;
    }
    100% {
      translate: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .switch,
    .switch::after {
      animation: none;
    }
    .switch {
      background: #0a84ff;
    }
    .switch::after {
      translate: 14px;
    }
  }
  .hint,
  .error {
    margin-top: 8px;
    font-size: 11.5px;
    color: var(--text-3);
  }
  .error {
    color: var(--warn);
  }
</style>
