<script lang="ts">
  import notices from "./third-party-notices.json";

  let { back }: { back: () => void } = $props();
  let expanded = $state<number | null>(null);
  const texts: Record<string, string> = notices.texts;
</script>

<div class="page">
  <button class="btn small" onclick={back}>Back to About</button>
  <h1>Third-party licences</h1>
  <ul class="group">
    {#each notices.packages as pkg, index}
      <li class="row">
        <button class="package" aria-expanded={expanded === index} onclick={() => (expanded = expanded === index ? null : index)}>
          <span class="label">{pkg.name} {pkg.version}</span>
          <span class="hint">{pkg.licence}</span>
        </button>
        {#if expanded === index}
          {#if pkg.fallback}
            <p class="hint">Standard SPDX text. This crate ships without a licence file.</p>
          {/if}
          {#each pkg.notices as notice}
            <h2>{notice.file}</h2>
            <pre>{texts[notice.text]}</pre>
          {/each}
        {/if}
      </li>
    {/each}
  </ul>
</div>

<style>
  h1 {
    margin: 12px 0;
  }
  ul {
    padding: 0;
  }
  li.row {
    display: block;
    padding: 0;
  }
  .package {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    align-items: baseline;
    gap: 4px 16px;
    width: 100%;
    padding: 10px 14px;
    text-align: left;
  }
  .package:hover {
    background: var(--bg-hover);
  }
  .package:focus-visible {
    outline-offset: -2px;
  }
  p, h2, pre {
    margin: 0 14px 12px;
  }
  h2 {
    font-size: 13px;
  }
  pre {
    padding: 10px;
    background: var(--bg-sunken);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: 12px;
  }
</style>
