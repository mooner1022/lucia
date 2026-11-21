<script lang="ts">
  import type { Snippet } from 'svelte';

  // 가로로 늘어놓기. 자리가 모자라면 다음 줄로 넘어간다(밖으로 밀려나지 않는다).
  let {
    gap = 2,
    justify = 'start',
    align = 'center',
    children,
  }: {
    gap?: 1 | 2 | 3 | 4 | 5;
    justify?: 'start' | 'between' | 'end';
    align?: 'center' | 'start' | 'end' | 'baseline' | 'stretch';
    children?: Snippet;
  } = $props();
</script>

<div class="cluster g{gap} j-{justify} a-{align}">{@render children?.()}</div>

<style>
  .cluster {
    display: flex;
    flex-wrap: wrap;
  }
  .g1 { gap: var(--s1); }
  .g2 { gap: var(--s2); }
  .g3 { gap: var(--s3); }
  .g4 { gap: var(--s4); }
  .g5 { gap: var(--s5); }
  .j-start { justify-content: flex-start; }
  .j-between { justify-content: space-between; }
  .j-end { justify-content: flex-end; }
  .a-center { align-items: center; }
  .a-start { align-items: flex-start; }
  .a-end { align-items: flex-end; }
  .a-baseline { align-items: baseline; }
  .a-stretch { align-items: stretch; }
</style>
