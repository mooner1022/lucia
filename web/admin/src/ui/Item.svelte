<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { Tone } from '../lib/types';
  import Icon from './Icon.svelte';

  // 목록의 한 줄: 앞 아이콘(결의 색) · 제목과 덧붙는 글 · 오른쪽 끝(배지, 단추) · 줄 아래 덧붙임(펼침).
  // 제목은 `title`(글) 또는 `heading`(마크업). children은 제목 아래 작은 글이다. 자리가 모자라면 끝 요소가 아래로 내려간다.
  // `href`를 주면 줄 전체가 그 화면으로 가는 링크다(끝에 화살표가 붙는다). 그때는 줄 안에 단추나 다른 링크를 두지 않는다.
  // 앞자리에 아이콘 대신 그림을 두려면 `media`에 `<Picture size="sm">`를 준다(제목 안에 넣으면 아래 글의 왼쪽 선이 어긋난다).
  // 줄 아래 덧붙임(extra)은 제목의 왼쪽 선에 맞춰 들여 쓴다(좁은 화면에서는 줄 전체 너비를 쓴다).
  let {
    title,
    heading,
    icon,
    media,
    tone = 'muted',
    href,
    end,
    extra,
    children,
  }: {
    title?: string;
    heading?: Snippet;
    icon?: string;
    media?: Snippet;
    tone?: Tone;
    href?: string;
    end?: Snippet;
    extra?: Snippet;
    children?: Snippet;
  } = $props();
</script>

{#snippet body()}
  {#if media}<span class="media">{@render media()}</span>{:else if icon}<span class="lead t-{tone}"><Icon name={icon} /></span>{/if}
  <div class="main">
    <div class="title">{#if heading}{@render heading()}{:else}{title}{/if}</div>
    {#if children}<div class="more">{@render children()}</div>{/if}
  </div>
  {#if end}<div class="end">{@render end()}</div>{/if}
{/snippet}

{#if href}
  <li class="linked">
    <a class="row" {href}>
      {@render body()}
      <span class="go"><Icon name="chevron" /></span>
    </a>
  </li>
{:else}
  <li class="row" class:led={!!icon && !media} class:pictured={!!media}>
    {@render body()}
    {#if extra}<div class="extra">{@render extra()}</div>{/if}
  </li>
{/if}

<style>
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
    gap: var(--s2) var(--s3);
    padding: var(--s3) 0;
  }
  li:first-child,
  li:first-child > a.row {
    padding-top: 0;
  }
  li:last-child,
  li:last-child > a.row {
    padding-bottom: 0;
  }
  a.row {
    flex-wrap: nowrap;
    color: inherit;
    text-decoration: none;
  }
  a.row:hover .title {
    color: var(--accent-text);
  }
  /* 링크 줄에 올리면 옅은 번짐이 깔린다. 줄의 크기·자리는 그대로이고(가상 요소가 글 양옆으로 조금 넓게 깔린다) 가로로 밀지 않는다.
     줄 사이는 이미 줄 안쪽 여백이라 위아래는 줄 칸까지만, 맨 위·맨 아래 줄만 바깥으로 조금 더 나간다(구역 카드의 안쪽 여백 쪽). */
  a.row {
    position: relative;
    isolation: isolate;
  }
  a.row::before {
    content: '';
    position: absolute;
    inset: 0 -0.5rem;
    z-index: -1;
    border-radius: var(--r-sm);
    background: transparent;
    pointer-events: none;
    transition: background-color 0.15s;
  }
  li:first-child > a.row::before {
    top: -0.4rem;
  }
  li:last-child > a.row::before {
    bottom: -0.4rem;
  }
  @media (hover: hover) {
    a.row:hover::before {
      background: color-mix(in srgb, var(--accent) 5%, transparent);
    }
  }
  .go {
    flex: none;
    align-self: center;
    color: var(--text-3);
  }
  /* 조건에 따라 안이 비면 칸을 그리지 않는다(빈 칸이 틈을 만들지 않게). */
  .more:empty,
  .end:empty,
  .extra:empty {
    display: none;
  }
  .lead {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 2rem;
    height: 2rem;
    border-radius: var(--r-full);
    background: var(--muted-soft);
    color: var(--muted);
  }
  .t-ok { background: var(--ok-soft); color: var(--ok); }
  .t-info { background: var(--info-soft); color: var(--info); }
  .t-warn { background: var(--warn-soft); color: var(--warn); }
  .t-bad { background: var(--bad-soft); color: var(--bad); }
  .media {
    display: inline-flex;
    flex: none;
  }
  /* 제목 칸의 바닥 너비: 이보다 좁아지면 끝 요소가 아래로 내려간다. 휴대폰 너비에서도 짧은 끝 요소 하나(시각, 단추 하나)는
     제목 옆에 남고, 단추가 둘셋이면 내려가는 값이다. */
  .main {
    display: flex;
    flex: 1 1 9rem;
    flex-direction: column;
    gap: 0.2rem;
  }
  .title {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--s1) var(--s2);
    font-weight: 600;
  }
  /* 제목 아래 글은 문장으로 흐른다. 줄을 나누려면 <p>를 쓴다. */
  .more {
    font-size: var(--fs-sm);
    color: var(--text-2);
  }
  .more :global(p + p) {
    margin-top: 0.2rem;
  }
  .end {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: var(--s2);
    margin-inline-start: auto;
    max-width: 100%;
  }
  .extra {
    display: flex;
    flex: 1 1 100%;
    flex-direction: column;
    gap: var(--s1);
  }
  /* 링크 줄의 끝 요소(시각)는 줄지 않고 한 줄로 둔다. */
  a.row > .end {
    flex: none;
    white-space: nowrap;
  }
  /* 덧붙임은 제목의 왼쪽 선에서 시작한다(앞 아이콘 2rem, 작은 그림 2.5rem + 사이 틈). 좁은 화면에서는 폼이 좁아지지 않게 들여 쓰지 않는다. */
  @media (min-width: 40rem) {
    .led > .extra {
      margin-inline-start: calc(2rem + var(--s3));
    }
    .pictured > .extra {
      margin-inline-start: calc(2.5rem + var(--s3));
    }
  }
</style>
