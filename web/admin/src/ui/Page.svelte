<script lang="ts">
  import type { Snippet } from 'svelte';

  import { siteName } from '../lib/app.svelte';
  import Icon from './Icon.svelte';

  // 화면 하나의 틀: 제목(h1), 한 줄 설명, 오른쪽 단추, 보조 탭, 그리고 구역들.
  // desc는 글, 마크업이 필요하면 `lead`를 쓴다. 하위 화면이면 `back`에 돌아갈 화면을 준다(제목 위의 링크).
  // 브라우저 탭의 제목은 title을 따른다. 같은 제목을 나눠 쓰는 화면(설정의 탭들)은 `docTitle`로 가른다.
  let {
    title,
    docTitle,
    desc,
    lead,
    back,
    actions,
    tabs,
    children,
  }: {
    title: string;
    docTitle?: string;
    desc?: string;
    lead?: Snippet;
    back?: { href: string; label: string };
    actions?: Snippet;
    tabs?: Snippet;
    children?: Snippet;
  } = $props();

  $effect(() => {
    document.title = `${docTitle ?? title} · ${siteName()}`;
  });
</script>

<div class="page">
  <div class="head">
  <header>
    <div class="text">
      {#if back}<a class="back" href={back.href}><Icon name="arrow-left" size="sm" /><span>{back.label}</span></a>{/if}
      <h1>{title}</h1>
      {#if lead}<p>{@render lead()}</p>{:else if desc}<p>{desc}</p>{/if}
    </div>
    {#if actions}<div class="actions">{@render actions()}</div>{/if}
  </header>
  <div class="rule" aria-hidden="true"></div>
  </div>
  {#if tabs}{@render tabs()}{/if}
  {@render children?.()}
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: var(--s5);
  }
  .head {
    display: flex;
    flex-direction: column;
    gap: var(--s3);
  }
  /* 화면의 동작 단추는 제목 옆 오른쪽에 놓인다(뒤로 가기 줄이 아니라 제목 덩어리의 가운데에 맞춘다). */
  header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--s3) var(--s5);
  }
  .text {
    display: flex;
    flex: 1 1 18rem;
    flex-direction: column;
    gap: var(--s1);
  }
  .back {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    width: fit-content;
    max-width: 100%;
    min-height: 1.75rem;
    color: var(--text-2);
    font-size: var(--fs-sm);
    font-weight: 600;
    text-decoration: none;
  }
  .back:hover {
    color: var(--accent-text);
  }
  /* 화면 제목만 제목 글꼴(명조)로 쓴다. */
  h1 {
    font-family: var(--font-display);
    font-size: var(--fs-2xl);
    font-weight: 700;
    letter-spacing: 0;
  }
  p {
    color: var(--text-2);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--s2);
  }
  /* 제목 아래의 맺음 줄: 꽃 한 송이에서 금선이 흘러 옅어진다. */
  .rule {
    display: flex;
    align-items: center;
    gap: var(--s2);
  }
  .rule::before {
    content: '';
    flex: none;
    width: 1rem;
    height: 1rem;
    background: var(--accent);
    -webkit-mask: url('./motif/blossom.svg') center / contain no-repeat;
    mask: url('./motif/blossom.svg') center / contain no-repeat;
  }
  .rule::after {
    content: '';
    flex: 1 1 auto;
    height: 1px;
    background: linear-gradient(90deg, var(--gold), transparent 80%);
  }
  @media (max-width: 40rem) {
    .page {
      gap: var(--s4);
    }
    h1 {
      font-size: var(--fs-xl);
    }
    /* 좁은 화면: 동작 단추를 제목·설명·맺음 줄 다음으로 내려 오른쪽에 붙인다(설명과 맺음 줄 사이에 끼지 않게).
       header의 칸을 풀어 .head가 직접 차례를 정한다. */
    header {
      display: contents;
    }
    /* 세로로 쌓일 때는 칸의 바닥 너비(18rem)가 높이로 읽히지 않게 푼다. */
    .text {
      flex: none;
      order: 1;
    }
    .rule {
      order: 2;
    }
    .actions {
      order: 3;
      justify-content: flex-end;
    }
  }
  @media (forced-colors: active) {
    .rule::before {
      display: none;
    }
    .rule::after {
      background: CanvasText;
    }
  }
</style>
