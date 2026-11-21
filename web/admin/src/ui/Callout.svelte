<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { Tone } from '../lib/types';
  import Icon from './Icon.svelte';
  import { TONE_ICON } from './tone';

  // 눈에 띄어야 하는 안내 상자.
  // `role`: 처리 결과처럼 바로 읽어 줘야 하는 안내면 "status"(실패는 "alert").
  let {
    tone = 'info',
    title,
    role,
    children,
  }: { tone?: Tone; title?: string; role?: 'status' | 'alert'; children?: Snippet } = $props();
</script>

<div class="callout {tone}" {role}>
  <span class="icon"><Icon name={TONE_ICON[tone]} /></span>
  <div class="text">
    {#if title}<strong>{title}</strong>{' '}{/if}{@render children?.()}
  </div>
</div>

<style>
  /* 결의 옅은 바탕과 같은 결의 옅은 테두리. 굵은 왼쪽 막대는 쓰지 않는다(결정 0062). */
  .callout {
    display: flex;
    gap: var(--s3);
    padding: var(--s3) var(--s4);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    background: var(--surface-2);
  }
  .icon {
    flex: none;
    color: var(--muted);
  }
  /* 글은 문장으로 흐른다(굵은 글·코드가 섞여도 줄이 끊기지 않는다). 문단을 나누려면 <p>를 쓴다. */
  .text {
    flex: 1 1 auto;
  }
  .text :global(p + p) {
    margin-top: var(--s2);
  }
  .ok { border-color: color-mix(in srgb, var(--ok) 35%, transparent); background: var(--ok-soft); }
  .ok .icon { color: var(--ok); }
  .info { border-color: color-mix(in srgb, var(--info) 35%, transparent); background: var(--info-soft); }
  .info .icon { color: var(--info); }
  .warn { border-color: color-mix(in srgb, var(--warn) 35%, transparent); background: var(--warn-soft); }
  .warn .icon { color: var(--warn); }
  .bad { border-color: color-mix(in srgb, var(--bad) 35%, transparent); background: var(--bad-soft); }
  .bad .icon { color: var(--bad); }
</style>
