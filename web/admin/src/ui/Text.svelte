<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { Tone } from '../lib/types';

  // 글 한 덩이. 흐린 글, 작은 글, 결이 있는 글에 쓴다.
  let {
    size,
    tone,
    weight = 'normal',
    mono = false,
    block = false,
    children,
  }: {
    /** 주지 않으면 놓인 자리의 글자 크기를 따른다(표, 목록의 작은 글 안에서 혼자 커지지 않는다). */
    size?: 'xs' | 'sm' | 'body' | 'lg';
    tone?: Tone | 'subtle';
    weight?: 'normal' | 'strong';
    mono?: boolean;
    block?: boolean;
    children?: Snippet;
  } = $props();
</script>

{#if block}
  <p class="{size ? `s-${size}` : ''} {tone ? `t-${tone}` : ''} w-{weight}" class:mono>{@render children?.()}</p>
{:else}
  <span class="{size ? `s-${size}` : ''} {tone ? `t-${tone}` : ''} w-{weight}" class:mono>{@render children?.()}</span>
{/if}

<style>
  .s-xs { font-size: var(--fs-xs); }
  .s-sm { font-size: var(--fs-sm); }
  .s-body { font-size: var(--fs-body); }
  .s-lg { font-size: var(--fs-lg); }
  .t-subtle { color: var(--text-2); }
  .t-muted { color: var(--text-3); }
  .t-ok { color: var(--ok); }
  .t-info { color: var(--info); }
  .t-warn { color: var(--warn); }
  .t-bad { color: var(--bad); }
  .w-strong { font-weight: 650; }
  .mono { font-family: var(--mono); font-size: 0.92em; }
</style>
