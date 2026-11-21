<script lang="ts">
  import { onMount } from 'svelte';

  // 같은 묶음의 화면을 오가는 탭(링크). 한 줄이고, 자리가 모자라면 그 줄 안에서 옆으로 민다.
  // 줄 밖에 가려진 칸이 있는 쪽 가장자리는 흐려져(data-more-start·data-more-end) 더 밀 수 있음을 알린다.
  let {
    label,
    items,
  }: { label: string; items: { href: string; label: string; current?: boolean; count?: number }[] } = $props();

  let nav = $state<HTMLElement>();
  let moreStart = $state(false);
  let moreEnd = $state(false);

  // 가장자리 흐림의 너비(CSS의 --fade와 같게). 지금 칸이 이 안에 걸리면 안 보이는 것으로 친다.
  const FADE_PX = 24;

  function measure() {
    if (!nav) return;
    const left = Math.abs(nav.scrollLeft);
    // 소수 픽셀 오차로 끝에서 흐림이 남지 않게 1px 여유를 둔다.
    moreStart = left > 1;
    moreEnd = nav.scrollWidth - nav.clientWidth - left > 1;
  }

  // 지금 칸이 가려져 있으면 그 줄 안에서만 옆으로 밀어 가운데에 둔다. 쪽 전체는 움직이지 않는다
  // (scrollIntoView는 위아래로도 밀 수 있어 scrollLeft를 직접 바꾼다).
  function revealCurrent() {
    const cur = nav?.querySelector<HTMLElement>('[aria-current="page"]');
    if (!nav || !cur) return;
    const box = nav.getBoundingClientRect();
    const at = cur.getBoundingClientRect();
    if (at.left >= box.left + FADE_PX && at.right <= box.right - FADE_PX) return;
    nav.scrollLeft += at.left + at.width / 2 - (box.left + box.width / 2);
    measure();
  }

  // 지금 칸이 바뀌면(탭을 눌러 같은 줄이 그대로 남은 채 화면만 바뀔 때) 다시 보이게 한다.
  const currentHref = $derived(items.find((i) => i.current)?.href);
  $effect(() => {
    void currentHref;
    revealCurrent();
  });

  // 글꼴이 들어오면 칸 너비가 바뀌니 한 번 더 맞춘다.
  onMount(() => {
    void document.fonts?.ready.then(revealCurrent);
  });

  // 칸이 바뀌거나 줄·칸의 크기가 바뀌면 다시 잰다. 시험 환경(jsdom)에는 ResizeObserver가 없다.
  $effect(() => {
    if (!nav) return;
    void items.map((i) => `${i.href}${i.label}${i.count ?? ''}`).join();
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const watch = new ResizeObserver(measure);
    watch.observe(nav);
    for (const a of nav.children) watch.observe(a);
    return () => watch.disconnect();
  });
</script>

<nav aria-label={label} bind:this={nav} onscroll={measure} data-more-start={moreStart || undefined} data-more-end={moreEnd || undefined}>
  {#each items as item (item.href)}
    <a href={item.href} aria-current={item.current ? 'page' : undefined}>
      <span>{item.label}</span>
      {#if item.count !== undefined}<span class="n">{item.count}</span>{/if}
    </a>
  {/each}
</nav>

<style>
  /* 알약 꼴 홈 안의 알약들(허리띠를 두른 옷감). 한 줄로 둔다. 자리가 모자라면 그 줄 안에서 옆으로 민다
     (두 줄로 꺾여 낱말 하나가 남는 것보다 낫다). 홈의 안쪽 여백은 초점 고리(3px + 간격 2px)가 홈 가장자리에 잘리지 않을 만큼 둔다. */
  nav {
    --fade: 1.5rem;
    display: flex;
    gap: var(--s1);
    overflow-x: auto;
    scrollbar-width: none;
    padding: 0.3rem;
    border: 1px solid var(--border);
    border-radius: var(--r-full);
    background: var(--surface-2);
    width: fit-content;
    max-width: 100%;
  }
  /* 가려진 칸이 있는 쪽 가장자리를 흐린다(스크롤 막대가 없어 밀 수 있다는 단서가 이것뿐이다). 마스크는 투명도만 본다.
     홈의 테두리·바탕도 같이 흐려져 줄이 그쪽으로 이어지는 모습이 된다. 인쇄에서는 끈다. */
  nav[data-more-start] {
    -webkit-mask-image: linear-gradient(to right, transparent, var(--text) var(--fade));
    mask-image: linear-gradient(to right, transparent, var(--text) var(--fade));
  }
  nav[data-more-end] {
    -webkit-mask-image: linear-gradient(to left, transparent, var(--text) var(--fade));
    mask-image: linear-gradient(to left, transparent, var(--text) var(--fade));
  }
  nav[data-more-start][data-more-end] {
    -webkit-mask-image: linear-gradient(to right, transparent, var(--text) var(--fade), var(--text) calc(100% - var(--fade)), transparent);
    mask-image: linear-gradient(to right, transparent, var(--text) var(--fade), var(--text) calc(100% - var(--fade)), transparent);
  }
  a {
    position: relative;
    display: inline-flex;
    flex: none;
    align-items: center;
    gap: var(--s2);
    white-space: nowrap;
    min-height: 2rem;
    padding: 0.2rem 0.9rem;
    border-radius: var(--r-full);
    color: var(--text-2);
    font-size: var(--fs-sm);
    font-weight: 600;
    text-decoration: none;
    transition:
      background-color 0.15s,
      color 0.15s,
      box-shadow 0.15s;
  }
  a:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  /* 지금 칸: 쪽빛 띠. 안쪽에 금선을 한 줄 두른다(Shell의 지금 화면 표시와 같은 꼴). 지금 칸이 없는 쪽(Jump)은 알약만 나란하다. */
  a[aria-current='page'] {
    background: linear-gradient(120deg, var(--band), var(--band-2));
    color: var(--on-band);
    font-weight: 650;
    box-shadow: var(--shadow-band);
  }
  a[aria-current='page']::after {
    content: '';
    position: absolute;
    inset: 3px;
    border: 1px solid var(--band-line);
    border-radius: inherit;
    opacity: 0.55;
    pointer-events: none;
  }
  .n {
    padding: 0 0.4rem;
    border-radius: var(--r-full);
    background: var(--surface-3);
    color: var(--text-2);
    font-size: var(--fs-xs);
  }
  a[aria-current='page'] .n {
    background: color-mix(in srgb, var(--on-band) 14%, transparent);
    color: var(--on-band);
  }
  @media (pointer: coarse) {
    a {
      min-height: 2.5rem;
    }
  }
  @media (forced-colors: active) {
    a[aria-current='page'] {
      outline: 2px solid Highlight;
      outline-offset: -2px;
    }
    a[aria-current='page']::after {
      display: none;
    }
  }
  /* 인쇄에서는 배경이 찍히지 않는다. 지금 칸의 흰 글이 사라지지 않게 한다. */
  @media print {
    nav,
    nav[data-more-start],
    nav[data-more-end],
    nav[data-more-start][data-more-end] {
      -webkit-mask-image: none;
      mask-image: none;
    }
    a[aria-current='page'] {
      border: 1px solid currentColor;
      background: none;
      color: inherit;
      box-shadow: none;
    }
    a[aria-current='page']::after {
      display: none;
    }
    a[aria-current='page'] .n {
      background: none;
      color: inherit;
    }
  }
</style>
