<script lang="ts">
  // "Try it now": the real suggestion panel, opened over this box the way it opens over text in any
  // app. ⌘E E here is caught by SuperEditr's shortcut watcher, which reports it as
  // `settings:review-shortcut`; applying sends the text back as `settings:playground-apply`.
  import { onMount } from "svelte";
  import { keycaps } from "../lib/accelerator";
  import type { Config } from "../lib/types";
  import { api } from "./ipc";

  interface Props {
    config: Config;
  }
  let { config }: Props = $props();

  let text = $state("Their going to love this, it work good and its realy fast. Lets ship it tommorow!");
  let box: HTMLTextAreaElement | undefined = $state();
  /** The part of `text` under review, replaced when the panel applies, and the whole text then:
   * if the box was edited meanwhile, the offsets no longer point at the reviewed words. */
  let range: [number, number] = [0, 0];
  let reviewed = "";
  let notice = $state("");

  function review() {
    if (!box || !text.trim()) return;
    const [start, end] = [box.selectionStart, box.selectionEnd];
    range = start === end ? [0, text.length] : [start, end];
    reviewed = text;
    notice = "";
    // The panel opens below this box; SuperEditr turns its frame in the window into screen points.
    const r = box.getBoundingClientRect();
    api.reviewPlayground(text.slice(range[0], range[1]), { x: r.left, y: r.top, width: r.width, height: r.height });
  }

  onMount(() => {
    const stops: (() => void)[] = [];
    api.on("settings:review-shortcut", () => document.activeElement === box && review()).then((s) => stops.push(s));
    api
      .on<string>("settings:playground-apply", (revised) => {
        if (text !== reviewed) {
          notice = "The text changed while it was being reviewed. Review it again.";
          return;
        }
        text = text.slice(0, range[0]) + revised + text.slice(range[1]);
      })
      .then((s) => stops.push(s));
    return () => stops.forEach((stop) => stop());
  });
</script>

<div class="playground">
  <textarea rows="2" bind:this={box} bind:value={text} spellcheck="false" aria-label="Text to try SuperEditr on"></textarea>
  <div class="controls">
    <span class="how">
      Select the text, then press {#each keycaps(config.hotkeys.review) as cap, i (i)}<kbd>{cap}</kbd>{/each}
    </span>
    <button class="btn small" onclick={review} disabled={!text.trim()}>Review</button>
  </div>
  {#if notice}<p class="notice">{notice}</p>{/if}
</div>

<style>
  .playground {
    display: grid;
    gap: 10px;
  }
  .controls {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
  }
  .how {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-2);
  }
  .notice {
    margin: 0;
    font-size: 12px;
    color: var(--warn);
  }
  .how kbd {
    font-size: 13px;
    padding: 2px 7px;
  }
</style>
