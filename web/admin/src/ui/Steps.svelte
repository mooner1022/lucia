<script lang="ts">
  import type { Snippet } from 'svelte';

  // 차례대로 하는 일(연결 절차). 안에는 Step만 둔다.
  let { children }: { children?: Snippet } = $props();
</script>

<ol>{@render children?.()}</ol>

<style>
  ol {
    display: flex;
    flex-direction: column;
    gap: var(--s4);
    padding: 0;
    list-style: none;
    counter-reset: step;
  }
  ol > :global(li + li) {
    margin-top: 0;
  }
</style>
