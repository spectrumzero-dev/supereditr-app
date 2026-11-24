<script lang="ts">
  import notices from "./third-party-notices.json";
  import { api } from "./ipc";

  let { back }: { back: () => void } = $props();
  let expanded = $state<number | null>(null);
  type Notice = { file: string; text: string; origin?: "spdx" };
  type Package = {
    name: string;
    version: string;
    licence: string;
    source: string;
    fallback: boolean;
    notices: Notice[];
  };
  const packages = notices.packages as Package[];
  const texts: Record<string, string> = notices.texts;
</script>

<div class="page">
  <button class="btn small" onclick={back}>Back to About</button>
  <h1>Third-party licences</h1>
  <ul class="group">
    {#each packages as pkg, index}
      <li class="row">
        <button class="package" aria-expanded={expanded === index} onclick={() => (expanded = expanded === index ? null : index)}>
          <span class="label">{pkg.name} {pkg.name === "WebView2 loader (Windows)" ? `(shipped with webview2-com-sys ${pkg.version})` : pkg.version}</span>
          <span class="hint">{pkg.licence}</span>
        </button>
        {#if expanded === index}
          <p class="hint">Source: <button class="linklike" onclick={() => api.openUrl(pkg.source)}>{pkg.source}</button></p>
          {#if pkg.fallback}
            <p class="hint">This package ships without a licence file.</p>
          {/if}
          {#each pkg.notices as notice}
            <h2>{notice.file}</h2>
            {#if notice.origin === "spdx"}
              <p class="hint">Standard SPDX text.</p>
            {/if}
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
    border: none;
    background: transparent;
    cursor: pointer;
  }
  .package:hover {
    background: var(--bg-hover);
  }
  .package:focus-visible {
    outline-offset: -2px;
  }
  .linklike {
    overflow-wrap: anywhere;
    text-align: left;
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
