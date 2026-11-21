<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { Tone } from '../lib/types';
  import Icon from './Icon.svelte';

  // 화면의 한 구역(카드). `id`는 화면 안에서 유일해야 한다(제목과 건너뛰기 링크가 쓴다).
  // 제목(아이콘·곁글 포함)은 카드 위 테두리에 걸친 쪽빛 띠로 그린다(결정 0062).
  // end: 제목 줄 오른쪽(배지, 단추). lead: 제목 아래 한 줄 설명. foot: 아래쪽 띠.
  let {
    id,
    title,
    icon,
    hint,
    tone,
    end,
    lead,
    foot,
    children,
  }: {
    id: string;
    title: string;
    icon?: string;
    hint?: string;
    tone?: Tone;
    end?: Snippet;
    lead?: Snippet;
    foot?: Snippet;
    children?: Snippet;
  } = $props();
</script>

<section {id} class={tone ? `tone-${tone}` : ''} aria-labelledby="{id}-title">
  <header>
    <div class="title-cell">
      <div class="ribbon">
        {#if icon}<span class="icon"><Icon name={icon} /></span>{/if}
        <div class="name">
          <h2 id="{id}-title">{title}</h2>
          {#if hint}<span class="hint">{hint}</span>{/if}
        </div>
      </div>
    </div>
    {#if end}<div class="end">{@render end()}</div>{/if}
  </header>
  {#if lead}<div class="lead">{@render lead()}</div>{/if}
  <div class="body">{@render children?.()}</div>
  {#if foot}<footer>{@render foot()}</footer>{/if}
</section>

<style>
  /* 구역 카드: 흰 옷감 한 폭. 제목은 카드 왼쪽 가장자리에서 나온 쪽빛 띠다(허리띠).
     띠와 오른쪽 것(배지, 링크, 단추)이 한 줄에 놓인다: 제목과 그 구역의 동작이 같은 높이에서 읽힌다. */
  section {
    --pad: var(--s5);
    position: relative;
    display: flex;
    flex-direction: column;
    gap: var(--s4);
    padding: var(--pad);
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
    box-shadow: var(--shadow-1);
    scroll-margin-top: 6rem;
  }
  .tone-warn {
    border-color: var(--warn);
  }
  .tone-bad {
    border-color: var(--bad);
  }
  header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--s2) var(--s4);
  }
  /* 제목 칸: 오른쪽 것(배지, 링크, 단추 하나)이 짧으면 좁은 화면에서도 같은 줄에 남는다. 칸이 10rem보다 좁아질 때만 오른쪽 것이 내려간다. */
  .title-cell {
    display: flex;
    flex: 1 1 10rem;
    margin-inline-start: calc(var(--pad) * -1 - 1px);
  }
  /* 띠는 카드의 왼쪽 테두리에 붙어 나오고 오른쪽 끝만 둥글다. 아이콘은 본문의 왼쪽 선에 맞는다.
     글이 길면 곁글이 제목 아래로 내려가고(아이콘 아래가 아니다), 띠가 아래로 자란다(높이를 정하지 않는다). */
  .ribbon {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--s2);
    max-width: 100%;
    min-height: 2rem;
    padding: 0.3rem 1.1rem 0.3rem var(--pad);
    border-radius: 0 1.05rem 1.05rem 0;
    background: linear-gradient(120deg, var(--band), var(--band-2));
    color: var(--on-band);
    box-shadow: var(--shadow-band);
    /* 그림자가 카드 왼쪽 밖으로 번지지 않게 왼쪽만 자른다. */
    clip-path: inset(-1rem -1rem -1rem 0);
  }
  /* 띠 안쪽의 금선 한 줄(왼쪽은 카드 밖으로 이어지는 꼴이라 긋지 않는다). */
  .ribbon::after {
    content: '';
    position: absolute;
    inset: 3px 3px 3px 0;
    border: 1px solid var(--band-line);
    border-inline-start: 0;
    border-radius: 0 0.85rem 0.85rem 0;
    opacity: 0.5;
    pointer-events: none;
  }
  .icon {
    display: inline-flex;
    flex: none;
    color: var(--band-line);
  }
  .name {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0 var(--s2);
  }
  h2 {
    font-size: var(--fs-body);
    font-weight: 650;
    letter-spacing: 0;
  }
  .hint {
    font-size: var(--fs-xs);
    font-weight: 500;
    color: var(--on-band-2);
  }
  /* 살펴야 하는 구역은 띠도 그 결의 색을 띤다(옅은 바탕에 결의 글·테두리. 쪽빛 띠보다 요란하지 않게). */
  .tone-warn .ribbon {
    border: 1px solid var(--warn);
    border-inline-start: 0;
    background: var(--warn-soft);
    color: var(--warn);
    box-shadow: none;
  }
  .tone-bad .ribbon {
    border: 1px solid var(--bad);
    border-inline-start: 0;
    background: var(--bad-soft);
    color: var(--bad);
    box-shadow: none;
  }
  .tone-warn .icon,
  .tone-bad .icon,
  .tone-warn .hint,
  .tone-bad .hint {
    color: inherit;
  }
  .tone-warn .ribbon::after,
  .tone-bad .ribbon::after {
    display: none;
  }
  .end {
    display: flex;
    flex: 0 1 auto;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: var(--s2);
    max-width: 100%;
    margin-inline-start: auto;
  }
  .lead {
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: var(--s4);
  }
  /* 소제목은 새 묶음의 시작이다: 위쪽을 더 띄우고 선을 그어, 앞 묶음이 아니라 뒤따르는 것과 붙어 보이게 한다. */
  .body > :global(h3:not(:first-child)) {
    margin-top: var(--s2);
    padding-top: var(--s4);
    border-top: 1px solid var(--border);
  }
  /* 접힌 펼침이 잇달아 놓이면 한 묶음으로 붙인다(목록 줄 안의 펼침과 같은 간격). */
  .body > :global(details:not([open]) + details:not([open])) {
    margin-top: calc(var(--s3) * -1);
  }
  /* 조건에 따라 안이 비면 띠와 칸을 그리지 않는다. */
  footer:empty,
  .end:empty,
  .lead:empty {
    display: none;
  }
  footer {
    margin: 0 calc(var(--pad) * -1) calc(var(--pad) * -1);
    padding: var(--s3) var(--pad);
    border-top: 1px solid var(--border);
    border-radius: 0 0 var(--r-lg) var(--r-lg);
    background: var(--surface-2);
    font-size: var(--fs-sm);
    color: var(--text-2);
  }
  @media (max-width: 40rem) {
    section {
      --pad: var(--s4);
      border-radius: var(--r-md);
    }
    footer {
      border-radius: 0 0 var(--r-md) var(--r-md);
    }
  }
  @media (forced-colors: active) {
    .ribbon {
      border: 1px solid CanvasText;
    }
    .ribbon::after {
      display: none;
    }
  }
  /* 인쇄에서는 배경이 찍히지 않는다. 띠의 흰 글이 사라지지 않게 글 색을 본문에 맞추고 테두리로 모양을 남긴다. */
  @media print {
    .ribbon,
    .tone-warn .ribbon,
    .tone-bad .ribbon {
      border: 1px solid currentColor;
      background: none;
      color: inherit;
      box-shadow: none;
    }
    .ribbon::after {
      display: none;
    }
    .icon,
    .hint {
      color: inherit;
    }
  }
</style>
