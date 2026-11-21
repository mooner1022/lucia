<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  // 절차의 한 단계: 번호(끝났으면 체크), 제목, 그 아래 내용(설명, 폼, 단추).
  // state: "done"(끝남) · "current"(지금 할 것) · "todo"(아직). 번호는 순서대로 저절로 붙는다.
  let {
    title,
    state = 'todo',
    children,
  }: { title: string; state?: 'done' | 'current' | 'todo'; children?: Snippet } = $props();
</script>

<li class={state} aria-current={state === 'current' ? 'step' : undefined}>
  <span class="mark" aria-hidden="true">{#if state === 'done'}<Icon name="check" size="sm" />{/if}</span>
  <div class="main">
    <div class="title">{title}{#if state === 'done'}<span class="sr-only"> (완료)</span>{/if}</div>
    {#if children}<div class="body">{@render children()}</div>{/if}
  </div>
</li>

<style>
  li {
    display: flex;
    align-items: flex-start;
    gap: var(--s3);
    counter-increment: step;
  }
  /* 번호는 원. 아직(todo)은 흰 바탕에 실선, 지금(current)은 쪽빛 띠 색, 끝남(done)은 비취. */
  .mark {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 1.75rem;
    height: 1.75rem;
    border: 1px solid var(--border-strong);
    border-radius: var(--r-full);
    background: var(--surface);
    color: var(--text-2);
    font-size: var(--fs-sm);
    font-weight: 700;
  }
  .todo .mark::before,
  .current .mark::before {
    content: counter(step);
  }
  .current .mark {
    border-color: transparent;
    background: linear-gradient(120deg, var(--band), var(--band-2));
    color: var(--on-band);
    box-shadow: var(--shadow-band);
  }
  .done .mark {
    border-color: color-mix(in srgb, var(--ok) 35%, transparent);
    background: var(--ok-soft);
    color: var(--ok);
  }
  .main {
    display: flex;
    flex: 1 1 auto;
    flex-direction: column;
    gap: var(--s2);
  }
  .title {
    min-height: 1.75rem;
    font-weight: 600;
    line-height: 1.75rem;
  }
  .todo .title {
    color: var(--text-2);
  }
  .current .title {
    font-weight: 650;
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: var(--s3);
  }
  .body:empty {
    display: none;
  }
  /* 강제 색 모드에서는 배경색이 사라진다. 지금 단계의 번호가 테두리와 시스템 색으로 남게 한다. */
  @media (forced-colors: active) {
    .current .mark {
      border-color: Highlight;
      background: Highlight;
      color: HighlightText;
    }
  }
  /* 인쇄에서는 배경이 찍히지 않는다. 지금 단계의 흰 번호가 사라지지 않게 한다. */
  @media print {
    .current .mark {
      border: 1px solid currentColor;
      background: none;
      color: inherit;
      box-shadow: none;
    }
  }
</style>
