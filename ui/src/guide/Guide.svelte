<script lang="ts">
  // Floats beside System Settings → Accessibility. SuperEditr is usually listed there already,
  // switched off: macOS lists any app that checks whether it has access, and SuperEditr must check.
  // So the card leads with the switch; dragging the tile (SuperEditr.app itself) is the fallback.
  // Dragging the tile drags SuperEditr.app itself
  // (a native file drag started in Rust), which the Accessibility list accepts.
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
  {#if error}<p class="error">{error}</p>{:else}<p class="hint">Not in the list? Drag this into it.</p>{/if}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="tile" onmousedown={drag} title="Drag into the list {where.words}">
    <img src="/icon.png" alt="" width="28" height="28" draggable="false" />
    <span>SuperEditr</span>
  </div>
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
    margin-top: 6px;
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
  .hint,
  .error {
    margin-top: 10px;
    font-size: 11.5px;
    color: var(--text-3);
  }
  .error {
    color: var(--warn);
  }
</style>
