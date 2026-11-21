<script lang="ts">
  import type { Snippet } from 'svelte';

  // 세로로 쌓기. 간격은 1(4px)~6(32px).
  let { gap = 3, children }: { gap?: 1 | 2 | 3 | 4 | 5 | 6; children?: Snippet } = $props();
</script>

<div class="stack g{gap}">{@render children?.()}</div>

<style>
  .stack {
    display: flex;
    flex-direction: column;
  }
  .g1 { gap: var(--s1); }
  .g2 { gap: var(--s2); }
  .g3 { gap: var(--s3); }
  .g4 { gap: var(--s4); }
  .g5 { gap: var(--s5); }
  .g6 { gap: var(--s6); }
</style>
