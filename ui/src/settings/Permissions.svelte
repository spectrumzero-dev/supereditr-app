<script lang="ts">
  import { api } from "./ipc";

  interface Props {
    accessibility: boolean;
  }
  let { accessibility }: Props = $props();
</script>

<div class="page">
  <h1>Permissions</h1>
  <p class="lede">What SuperEditr needs from macOS, and why.</p>

  <div class="group">
    <div class="row">
      <div>
        <div class="label">Accessibility {#if accessibility}<span class="pill good">Granted</span>{:else}<span class="pill bad">Needed</span>{/if}</div>
        <div class="hint">Lets SuperEditr read the text you select and replace it when you accept a suggestion. Password fields are never read.</div>
      </div>
      {#if !accessibility}
        <button class="btn primary" onclick={() => api.guideAccessibility()}>Grant access</button>
      {/if}
    </div>
  </div>
</div>
