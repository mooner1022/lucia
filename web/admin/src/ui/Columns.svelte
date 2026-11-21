<script lang="ts">
  import type { Snippet } from 'svelte';

  // 구역(Section)이 여럿인 화면의 배치. 넓으면 두 단으로 나누되 두 단의 높이를 저절로 맞춘다
  // (한쪽 단만 길어져 옆이 비는 일이 없다). 좁으면 한 단이다. 구역은 위에서 아래로, 그다음 옆 단으로 이어진다.
  // 안에는 구역만 둔다. 구역 하나는 단 사이에서 쪼개지지 않는다.
  //
  // **한 번 나눈 단은 그대로 둔다.** 높이 맞추기를 브라우저에만 맡기면 접힌 것을 펼치거나 입력 칸을 늘릴 때마다
  // 다시 나눠서, 쓰고 있던 구역이 옆 단으로 옮겨 간다. 그래서 처음 그릴 때(그리고 너비나 구역 수가 바뀔 때)의
  // 나눔을 기억해, 둘째 단의 첫 구역 앞에서 단을 끊도록 못 박는다.
  let { children }: { children?: Snippet } = $props();
  let root: HTMLDivElement | undefined = $state();

  $effect(() => {
    const el = root;
    if (!el || typeof ResizeObserver === 'undefined') return;
    const pin = () => {
      const kids = [...el.children] as HTMLElement[];
      for (const kid of kids) kid.removeAttribute('data-column-start');
      if (kids.length < 2) return;
      // 못을 뺀 상태의 자연스러운 나눔을 읽는다(읽을 때 브라우저가 배치를 다시 계산한다).
      const first = kids[0].getBoundingClientRect().left;
      kids.find((kid) => kid.getBoundingClientRect().left > first + 1)?.setAttribute('data-column-start', '');
    };
    let width = -1;
    const resize = new ResizeObserver((entries) => {
      const now = Math.round(entries[0].contentRect.width);
      if (now !== width) {
        width = now;
        pin();
      }
    });
    resize.observe(el);
    const change = new MutationObserver(pin);
    change.observe(el, { childList: true });
    return () => {
      resize.disconnect();
      change.disconnect();
    };
  });
</script>

<div class="columns" bind:this={root}>{@render children?.()}</div>

<style>
  .columns {
    columns: 2 26rem;
    column-gap: var(--s5);
  }
  .columns > :global(*) {
    break-inside: avoid;
    margin-bottom: var(--s5);
  }
  /* 각 단의 마지막 구역은 아래 여백이 없다(첫 단이 더 길 때 그 여백이 통의 높이에 더해져 아래가 벌어지던 것). */
  .columns > :global(*:last-child),
  .columns > :global(*:has(+ [data-column-start])) {
    margin-bottom: 0;
  }
  .columns > :global([data-column-start]) {
    break-before: column;
  }
  @media (max-width: 40rem) {
    .columns > :global(*) {
      margin-bottom: var(--s4);
    }
  }
</style>
