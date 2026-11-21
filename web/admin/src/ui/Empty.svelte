<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  // 목록이 비었을 때의 한 줄.
  let { icon = 'info', children }: { icon?: string; children?: Snippet } = $props();
</script>

<p><Icon name={icon} /><span>{@render children?.()}</span></p>

<style>
  p {
    display: flex;
    align-items: flex-start;
    gap: var(--s2);
    padding: var(--s3) var(--s4);
    border: 1px dashed color-mix(in srgb, var(--accent) 35%, transparent);
    border-radius: var(--r-md);
    background: var(--surface-2);
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  p > :global(svg) {
    margin-top: 0.2em;
    color: var(--accent-text);
  }
</style>
