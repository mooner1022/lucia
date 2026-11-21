<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { Tone } from '../lib/types';
  import Icon from './Icon.svelte';
  import { TONE_ICON } from './tone';

  // 상태 배지. 색만으로 알리지 않게 아이콘과 글이 함께 간다. `plain`이면 아이콘 없이.
  let { tone = 'muted', plain = false, children }: { tone?: Tone; plain?: boolean; children?: Snippet } = $props();
</script>

<span class="badge {tone}">
  {#if !plain}<Icon name={TONE_ICON[tone]} size="sm" />{/if}
  <span>{@render children?.()}</span>
</span>

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    max-width: 100%;
    padding: 0.1rem 0.55rem;
    /* 같은 결의 옅은 테두리(글자색을 섞는다). */
    border: 1px solid color-mix(in srgb, currentColor 22%, transparent);
    border-radius: var(--r-full);
    background: var(--muted-soft);
    color: var(--text-2);
    font-size: var(--fs-xs);
    font-weight: 600;
    line-height: 1.5;
    text-align: start;
  }
  .ok { background: var(--ok-soft); color: var(--ok); }
  .info { background: var(--info-soft); color: var(--info); }
  .warn { background: var(--warn-soft); color: var(--warn); }
  .bad { background: var(--bad-soft); color: var(--bad); }
  @media (forced-colors: active) {
    .badge {
      border: 1px solid CanvasText;
    }
  }
</style>
