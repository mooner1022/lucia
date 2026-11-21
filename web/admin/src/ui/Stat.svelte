<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { Tone } from '../lib/types';
  import Dot from './Dot.svelte';
  import Icon from './Icon.svelte';

  // 상태 타일: 이름(앞에 아이콘이나 결의 점), 큰 값, 덧붙는 한 줄. `href`를 주면 타일 전체가 링크다.
  // 여러 개를 나란히 둘 때는 `<Grid min="xs" stretch>` 안에 둔다(높이가 맞는다).
  let {
    label,
    value,
    tone,
    icon,
    href,
    children,
  }: { label: string; value: string; tone?: Tone; icon?: string; href?: string; children?: Snippet } = $props();
</script>

{#snippet inner()}
  <span class="label">{#if icon}<Icon name={icon} />{/if}{label}{#if tone}<Dot {tone} />{/if}</span>
  <strong class="value">{value}</strong>
  {#if children}<span class="note">{@render children()}</span>{/if}
{/snippet}

{#if href}
  <a class="stat" {href}>{@render inner()}</a>
{:else}
  <div class="stat">{@render inner()}</div>
{/if}

<style>
  .stat {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    padding: var(--s3) var(--s4);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    /* 흰 타일의 아래쪽이 옅게 푸르다(치마의 번짐). */
    background: var(--surface);
    color: inherit;
    text-decoration: none;
    transition: border-color 0.15s, box-shadow 0.15s;
  }
  a.stat:hover {
    border-color: var(--accent);
    box-shadow: var(--shadow-band);
  }
  .label {
    display: flex;
    align-items: center;
    gap: var(--s2);
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .value {
    font-size: var(--fs-lg);
    font-weight: 700;
    line-height: 1.3;
  }
  .note {
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
</style>
