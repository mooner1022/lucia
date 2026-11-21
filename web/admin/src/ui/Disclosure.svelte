<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  // 접어 두는 것. kind="about": 긴 설명("자세히"). kind="more": 고급 설정이나 덧붙는 폼(테두리 있는 상자).
  let {
    summary,
    kind = 'about',
    open = false,
    children,
  }: { summary: string; kind?: 'about' | 'more'; open?: boolean; children?: Snippet } = $props();
</script>

<details class={kind} {open}>
  <summary><Icon name="chevron" size="sm" /><span>{summary}</span></summary>
  <div class="body">{@render children?.()}</div>
</details>

<style>
  summary {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    max-width: 100%;
    min-height: 1.75rem;
    color: var(--accent-text);
    font-size: var(--fs-sm);
    font-weight: 600;
    cursor: pointer;
    list-style: none;
  }
  summary::-webkit-details-marker {
    display: none;
  }
  summary > :global(svg) {
    transition: transform 0.15s;
  }
  details[open] > summary > :global(svg) {
    transform: rotate(90deg);
  }
  /* 안의 것은 세로로 쌓인다. 글은 <p>로 감싼다(감싸지 않은 글에 굵은 글이 섞이면 줄이 끊긴다). */
  .body {
    display: flex;
    flex-direction: column;
    gap: var(--s3);
    margin-top: var(--s2);
  }
  .about .body {
    padding-inline-start: var(--s4);
    border-inline-start: 2px solid var(--border);
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  /* 접혀 있을 때는 글 한 줄로 가볍게 보이고, 펼치면 안의 폼을 상자로 묶는다. */
  .more > summary {
    color: var(--text-2);
  }
  .more[open] {
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    background: var(--surface-2);
  }
  .more[open] > summary {
    display: flex;
    padding: var(--s2) var(--s3);
    color: var(--text);
  }
  .more .body {
    margin-top: 0;
    padding: var(--s3) var(--s4) var(--s4);
    border-top: 1px solid var(--border);
  }
  @media (pointer: coarse) {
    summary {
      min-height: 2.75rem;
    }
  }
</style>
