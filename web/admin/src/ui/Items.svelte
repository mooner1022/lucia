<script lang="ts">
  import type { Snippet } from 'svelte';

  // 줄 목록. 안에는 Item만 둔다.
  let { children }: { children?: Snippet } = $props();
</script>

<ul>{@render children?.()}</ul>

<style>
  ul {
    display: flex;
    flex-direction: column;
    padding: 0;
    list-style: none;
  }
  ul > :global(li + li) {
    margin-top: 0;
    border-top: 1px solid var(--border);
  }
</style>
