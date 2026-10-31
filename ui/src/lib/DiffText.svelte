<script lang="ts">
  import type { DiffView } from "./types";
  import { changeLabel } from "./diff";

  interface Props {
    view: DiffView;
    rejected: Set<number>;
    focused?: number | null;
    streaming?: boolean;
    ontoggle?: (id: number) => void;
  }
  let { view, rejected, focused = null, streaming = false, ontoggle }: Props = $props();
</script>

<div class="diff" class:streaming>
  {#each view.segments as segment, i (segment.type === "change" ? `c${segment.id}` : `e${i}`)}
    {#if segment.type === "equal"}<span>{segment.text}</span>{:else}<button
        type="button"
        class="change {segment.kind}"
        class:rejected={rejected.has(segment.id)}
        class:focused={focused === segment.id}
        title={`${changeLabel(segment)}${segment.note ? ` — ${segment.note}` : ""} (click to ${rejected.has(segment.id) ? "restore" : "keep original"})`}
        tabindex="-1"
        onclick={() => ontoggle?.(segment.id)}
        >{#if rejected.has(segment.id)}<span class="kept">{segment.old}</span>{:else}{#if segment.old.trim()}<del>{segment.old}</del>{/if}{#if segment.new}<ins>{segment.new}</ins>{/if}{/if}</button
      >{/if}
  {/each}{#if view.pending}<span class="pending">{view.pending}</span>{/if}
</div>

<style>
  .diff {
    white-space: pre-wrap;
    word-wrap: break-word;
    line-height: 1.6;
    font-size: 14px;
  }
  .change {
    all: unset;
    cursor: pointer;
    border-radius: 4px;
    padding: 0 1px;
    transition: box-shadow 120ms ease, background 120ms ease;
  }
  .change:hover {
    background: var(--bg-hover);
  }
  .change.focused {
    box-shadow: var(--focus-ring);
    background: var(--accent-soft);
  }
  del {
    color: var(--del);
    background: var(--del-bg);
    text-decoration: line-through;
    text-decoration-thickness: 1.5px;
    border-radius: 3px;
    padding: 0 1px;
    margin-right: 1px;
  }
  ins {
    color: var(--ins);
    background: var(--ins-bg);
    text-decoration: none;
    border-radius: 3px;
    padding: 0 1px;
    font-weight: 550;
  }
  .kept {
    text-decoration: underline dotted var(--text-3);
    text-underline-offset: 3px;
  }
  .pending {
    color: var(--text-3);
  }
  .streaming .pending {
    background: linear-gradient(90deg, var(--text-3) 0%, var(--text) 20%, var(--text-3) 40%) 0 0 / 300% 100%;
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
    animation: shimmer 1.4s linear infinite;
  }
  @keyframes shimmer {
    from {
      background-position: 100% 0;
    }
    to {
      background-position: -200% 0;
    }
  }
</style>
