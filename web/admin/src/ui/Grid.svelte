<script lang="ts">
  import type { Snippet } from 'svelte';

  // 칸 수를 정하지 않는 격자: 칸의 가장 좁은 너비(min)만 정하면 자리에 맞는 만큼 들어간다.
  // 좁은 화면에서는 저절로 한 줄에 한 칸이 된다. `stretch`면 한 줄의 칸 높이를 맞춘다(타일).
  let {
    min = 'md',
    gap = 4,
    stretch = false,
    children,
  }: { min?: 'xs' | 'sm' | 'md' | 'lg'; gap?: 2 | 3 | 4 | 5; stretch?: boolean; children?: Snippet } = $props();
</script>

<div class="grid m-{min} g{gap}" class:stretch>{@render children?.()}</div>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, var(--min)), 1fr));
    align-items: start;
  }
  .stretch {
    align-items: stretch;
  }
  .m-xs { --min: 9rem; }
  .m-sm { --min: 14rem; }
  .m-md { --min: 20rem; }
  .m-lg { --min: 28rem; }
  .g2 { gap: var(--s2); }
  .g3 { gap: var(--s3); }
  .g4 { gap: var(--s4); }
  .g5 { gap: var(--s5); }
</style>
