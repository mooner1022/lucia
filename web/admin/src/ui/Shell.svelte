<script lang="ts">
  import type { Snippet } from 'svelte';
  import { app } from '../lib/app.svelte';
  import { NAV, QUICK, isCurrent, type NavPage } from '../lib/nav';
  import { THEME_ICON, THEME_LABEL, cycleTheme, theme } from '../lib/theme.svelte';
  import type { PageData } from '../lib/types';
  import ActionForm from './ActionForm.svelte';
  import Button from './Button.svelte';
  import Icon from './Icon.svelte';
  import Petals from './Petals.svelte';
  import { TONE_ICON as toneIcon } from './tone';

  // 로그인한 화면의 틀: 건너뛰기 링크, 사이드바(넓은 화면), 위 막대와 아래 막대·펼침 메뉴(좁은 화면),
  // 처리 결과 안내, 본문. `wide`는 본문을 가득 쓰는 화면(화면 넘겨받기).
  // 모습(결정 0062): 사이드바는 치마(위는 흰 옷감, 아래는 푸른 번짐과 흰 꽃), 지금 화면은 쪽빛 띠,
  // 본문 위쪽은 하늘 번짐과 흩날리는 꽃잎(Petals), 좁은 화면의 위 막대는 쪽빛 띠다. 장식은 글·조작 뒤에 그려지고 누를 수 없다.
  let { data, wide = false, children }: { data: PageData; wide?: boolean; children?: Snippet } = $props();

  const all = NAV.flatMap((g) => g.pages);
  const quick = QUICK.map((key) => all.find((p) => p.key === key)).filter((p): p is NavPage => !!p);
  let menu: HTMLDialogElement | undefined = $state();

  function openMenu() {
    if (menu && typeof menu.showModal === 'function') menu.showModal();
  }
  function closeMenu() {
    if (menu?.open) menu.close();
  }
  function menuClick(event: MouseEvent) {
    const target = event.target as Element;
    if (target === menu || target.closest('a')) closeMenu();
  }
</script>

{#snippet brand()}
  <a class="brand" href="/">
    <span class="mark"><Icon name="blossom" /></span>
    <span class="name">{data.name} <span class="sub">관리</span></span>
  </a>
{/snippet}

{#snippet state()}
  <a class="state {data.overall.tone}" href="/" title="홈에서 자세히 봅니다">
    <Icon name={toneIcon[data.overall.tone]} />
    <span>전체 상태: {data.overall.label}{data.notices.length > 0 ? ` · ${data.notices.length}건` : ''}</span>
  </a>
{/snippet}

{#snippet links()}
  <div class="nav">
    {#each NAV as group (group.title)}
      <div class="group">
        {#if group.title}<div class="group-title">{group.title}</div>{/if}
        {#each group.pages as item (item.key)}
          <a class="link" href={item.path} aria-current={isCurrent(item, data.page) ? 'page' : undefined}>
            <Icon name={item.icon} />
            <span>{item.label}</span>
            {#if item.key === 'activity' && data.pending > 0}
              <span class="count" title="승인 대기">{data.pending}<span class="sr-only">건 승인 대기</span></span>
            {/if}
          </a>
        {/each}
      </div>
    {/each}
  </div>
{/snippet}

{#snippet foot()}
  <div class="foot">
    <Button variant="ghost" size="sm" icon={THEME_ICON[theme.value]} onclick={cycleTheme}>화면: {THEME_LABEL[theme.value]}</Button>
    <ActionForm action="/logout" inline track={false}>
      <Button type="submit" variant="ghost" size="sm" icon="logout">로그아웃</Button>
    </ActionForm>
  </div>
{/snippet}

<a class="skip" href="#main">본문으로 건너뛰기</a>
{#if app.loading}<div class="progress" role="progressbar" aria-label="불러오는 중"></div>{/if}
<div class="shell">
  <aside class="side">
    <div class="side-scroll">
      {@render brand()}
      {@render state()}
      <nav aria-label="주 메뉴">{@render links()}</nav>
      {@render foot()}
    </div>
  </aside>
  <div class="column">
    <Petals />
    <header class="top">
      {@render brand()}
      <span class="spacer"></span>
      <Button size="sm" icon="menu" aria-haspopup="dialog" onclick={openMenu}>메뉴</Button>
    </header>
    <main id="main" tabindex="-1" class:wide>
      {#if app.flash}
        <div class="flash {app.flash.tone}" role={app.flash.tone === 'bad' ? 'alert' : 'status'}>
          <Icon name={toneIcon[app.flash.tone]} />
          <p>{app.flash.text}</p>
          <button type="button" class="close" aria-label="안내 닫기" onclick={() => (app.flash = null)}><Icon name="x" /></button>
        </div>
      {/if}
      {@render children?.()}
      <div class="footer">
        <span class="bud" aria-hidden="true"></span>
        <p class="motto">구름처럼, 조용히 흐르고 달빛처럼, 은은히 비추며…</p>
        <p>{data.name} · 관리 페이지</p>
      </div>
    </main>
  </div>
</div>
<nav class="bottom" aria-label="빠른 메뉴">
  {#each quick as item (item.key)}
    <a href={item.path} aria-current={isCurrent(item, data.page) ? 'page' : undefined}>
      <Icon name={item.icon} size="lg" />
      <span>{item.label}</span>
      {#if item.key === 'activity' && data.pending > 0}<span class="pip" aria-hidden="true"></span><span class="sr-only">(승인 대기 {data.pending}건)</span>{/if}
    </a>
  {/each}
  <button type="button" aria-haspopup="dialog" onclick={openMenu}><Icon name="menu" size="lg" /><span>전체</span></button>
</nav>
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<dialog class="drawer" bind:this={menu} aria-label="메뉴" onclick={menuClick}>
  <div class="drawer-box">
    <div class="drawer-head">
      {@render brand()}
      <Button variant="ghost" size="sm" icon="x" onclick={closeMenu}>닫기</Button>
    </div>
    {@render state()}
    <nav aria-label="주 메뉴(좁은 화면)">{@render links()}</nav>
    {@render foot()}
  </div>
</dialog>

<style>
  .skip {
    position: absolute;
    inset-inline-start: var(--s3);
    top: -4rem;
    z-index: 100;
    padding: var(--s2) var(--s4);
    border-radius: var(--r-full);
    background: var(--accent);
    color: var(--on-accent);
    font-weight: 600;
  }
  .skip:focus {
    top: var(--s3);
  }
  .progress {
    position: fixed;
    inset: 0 0 auto 0;
    z-index: 90;
    height: 3px;
    background: linear-gradient(90deg, transparent, var(--accent), var(--band-line), transparent);
    background-size: 40% 100%;
    background-repeat: no-repeat;
    animation: slide 1s linear infinite;
  }
  @keyframes slide {
    from { background-position: -40% 0; }
    to { background-position: 140% 0; }
  }
  .shell {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    min-height: 100dvh;
  }
  .side {
    display: none;
  }
  /* 본문 쪽: 위에서부터 하늘빛이 번지고 오른쪽 위에 빛이 걸린다. 그 위로 꽃잎이 흩날린다(Petals). */
  .column {
    position: relative;
    display: flex;
    flex-direction: column;
    overflow: clip;
    background:
      radial-gradient(34rem 18rem at 92% -4rem, var(--glow), transparent 72%),
      linear-gradient(180deg, var(--sky), var(--bg) 22rem);
    background-repeat: no-repeat;
  }
  /* 좁은 화면의 위 막대: 쪽빛 띠와 금선. */
  .top {
    position: sticky;
    top: 0;
    z-index: 30;
    display: flex;
    align-items: center;
    gap: var(--s3);
    padding: var(--s2) var(--s4);
    padding-top: max(var(--s2), env(safe-area-inset-top));
    border-bottom: 1px solid var(--band-line);
    background: linear-gradient(120deg, var(--band), var(--band-2));
    box-shadow: var(--shadow-band);
  }
  /* 띠 위에 놓인 것의 초점 고리는 금선 색이다(--focus는 쪽빛 위에서 보이지 않는다). */
  .top .brand,
  .top :global(.btn) {
    outline-color: var(--band-line);
  }
  .top .brand {
    color: var(--on-band);
  }
  .top .sub {
    color: var(--on-band-2);
  }
  .top .mark {
    box-shadow: inset 0 0 0 1px var(--band-line);
  }
  .spacer {
    flex: 1 1 auto;
  }
  main {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: var(--s5);
    width: 100%;
    max-width: var(--content-w);
    margin-inline: auto;
    padding: var(--s5) var(--s4) calc(var(--bottomnav-h) + var(--s6) + env(safe-area-inset-bottom));
    outline: none;
  }
  main.wide {
    max-width: none;
  }
  .brand {
    display: inline-flex;
    align-items: center;
    gap: var(--s3);
    min-width: 0;
    color: var(--text);
    text-decoration: none;
  }
  /* 이름은 사용자가 정한다(결정 0064, 24자까지). 좁은 자리에서는 줄을 바꾸고, 끊을 곳 없는 긴 이름은 어디서든 끊는다. */
  .name {
    min-width: 0;
    overflow-wrap: anywhere;
    font-family: var(--font-display);
    font-size: var(--fs-xl);
    font-weight: 700;
    line-height: 1.2;
    letter-spacing: 0.01em;
  }
  /* 이름 표식: 쪽빛 바탕의 흰 꽃. 어두운 판에서도 같은 색이다(시험이 이 규칙의 그라데이션과 글 색의 대비를 본다). */
  .mark {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 2.25rem;
    height: 2.25rem;
    border-radius: var(--r-full);
    background: linear-gradient(135deg, #2f3f96, #4a60c0);
    color: #fff;
    box-shadow: var(--shadow-band);
  }
  .mark > :global(svg) {
    width: 1.3rem;
    height: 1.3rem;
  }
  .sub {
    margin-inline-start: 0.15em;
    color: var(--text-3);
    font-family: var(--font);
    font-size: var(--fs-sm);
    font-weight: 600;
    letter-spacing: 0;
  }
  .state {
    display: flex;
    align-items: center;
    gap: var(--s2);
    padding: var(--s2) var(--s3);
    border-radius: var(--r-md);
    background: var(--ok-soft);
    color: var(--ok);
    font-size: var(--fs-sm);
    font-weight: 600;
    text-decoration: none;
  }
  .state.warn { background: var(--warn-soft); color: var(--warn); }
  .state.bad { background: var(--bad-soft); color: var(--bad); }
  .nav {
    display: flex;
    flex-direction: column;
    gap: var(--s4);
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .group-title {
    padding: 0 var(--s4) var(--s1);
    color: var(--text-2);
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.04em;
  }
  .link {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--s3);
    min-height: 2.5rem;
    padding: 0.3rem var(--s4);
    border-radius: var(--r-full);
    color: var(--text-2);
    font-weight: 550;
    text-decoration: none;
    transition: background-color 0.15s;
  }
  .link:hover {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    color: var(--text);
  }
  /* 지금 화면: 쪽빛 띠. 안쪽에 금선을 한 줄 두른다. */
  .link[aria-current='page'] {
    background: linear-gradient(120deg, var(--band), var(--band-2));
    color: var(--on-band);
    font-weight: 650;
    box-shadow: var(--shadow-band);
  }
  .link[aria-current='page']::after {
    content: '';
    position: absolute;
    inset: 3px;
    border: 1px solid var(--band-line);
    border-radius: inherit;
    opacity: 0.55;
    pointer-events: none;
  }
  .link[aria-current='page'] > :global(svg) {
    color: var(--band-line);
  }
  .link > span:first-of-type {
    flex: 1 1 auto;
  }
  .count {
    padding: 0 0.45rem;
    border-radius: var(--r-full);
    background: var(--warn-soft);
    color: var(--warn);
    font-size: var(--fs-xs);
    font-weight: 700;
  }
  .foot {
    display: flex;
    flex-wrap: wrap;
    gap: var(--s1);
    margin-top: auto;
    padding-top: var(--s3);
  }
  .flash {
    display: flex;
    align-items: flex-start;
    gap: var(--s3);
    padding: var(--s3) var(--s4);
    border: 1px solid var(--info);
    border-radius: var(--r-md);
    background: var(--info-soft);
    color: var(--text);
    box-shadow: var(--shadow-1);
  }
  .flash > :global(svg) {
    margin-top: 0.25em;
    color: var(--info);
  }
  .flash p {
    flex: 1 1 auto;
  }
  .flash.ok { border-color: var(--ok); background: var(--ok-soft); }
  .flash.ok > :global(svg) { color: var(--ok); }
  .flash.warn { border-color: var(--warn); background: var(--warn-soft); }
  .flash.warn > :global(svg) { color: var(--warn); }
  .flash.bad { border-color: var(--bad); background: var(--bad-soft); }
  .flash.bad > :global(svg) { color: var(--bad); }
  .close {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 1.75rem;
    height: 1.75rem;
    border: 0;
    border-radius: var(--r-full);
    background: transparent;
    color: var(--text-2);
    cursor: pointer;
  }
  .close:hover {
    background: var(--surface-3);
  }
  /* 맺음: 꽃 한 송이와 양옆의 금선, 캐릭터 시트의 글귀. */
  .footer {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--s1);
    padding-top: var(--s4);
    /* 꽃잎이 뒤로 지나갈 수 있어 흐린 글(--text-3)보다 한 단 진하게 쓴다. */
    color: var(--text-2);
    font-size: var(--fs-xs);
    text-align: center;
  }
  .bud {
    position: relative;
    width: 1.1rem;
    height: 1.1rem;
    background: var(--accent);
    opacity: 0.55;
    -webkit-mask: url('./motif/blossom.svg') center / contain no-repeat;
    mask: url('./motif/blossom.svg') center / contain no-repeat;
  }
  .motto {
    font-family: var(--font-display);
    font-size: var(--fs-sm);
    letter-spacing: 0.02em;
  }
  .bottom {
    position: fixed;
    inset: auto 0 0 0;
    z-index: 30;
    display: grid;
    grid-template-columns: repeat(5, minmax(0, 1fr));
    padding-bottom: env(safe-area-inset-bottom);
    border-top: 1px solid var(--border);
    background: var(--surface);
    box-shadow: var(--shadow-1);
  }
  .bottom a,
  .bottom button {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.1rem;
    min-height: var(--bottomnav-h);
    padding: var(--s1);
    border: 0;
    background: transparent;
    color: var(--text-3);
    font-size: var(--fs-xs);
    font-weight: 600;
    line-height: 1.3;
    text-decoration: none;
    cursor: pointer;
  }
  .bottom a[aria-current='page'] {
    color: var(--accent-text);
  }
  /* 지금 화면: 위쪽에 쪽빛 띠 한 토막. */
  .bottom a[aria-current='page']::before {
    content: '';
    position: absolute;
    top: 0;
    width: 2.75rem;
    height: 3px;
    border-radius: 0 0 var(--r-full) var(--r-full);
    background: linear-gradient(90deg, var(--band), var(--band-2));
  }
  .pip {
    position: absolute;
    top: 0.55rem;
    inset-inline-start: calc(50% + 0.55rem);
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 50%;
    background: var(--warn);
  }
  .side,
  .drawer {
    background: linear-gradient(180deg, var(--surface) 0%, var(--surface) 36%, var(--wash) 66%, var(--hem) 100%);
  }
  .drawer {
    width: min(21rem, calc(100vw - 2.5rem));
    max-width: none;
    height: 100dvh;
    max-height: none;
    margin: 0 0 0 auto;
    padding: 0;
    border: 0;
    border-inline-start: 1px solid var(--border);
    color: var(--text);
    overflow-y: auto;
  }
  .drawer::backdrop {
    background: var(--scrim);
  }
  .drawer-box {
    position: relative;
    isolation: isolate;
    display: flex;
    flex-direction: column;
    gap: var(--s4);
    min-height: 100%;
    padding: var(--s4);
    padding-bottom: max(var(--s4), env(safe-area-inset-bottom));
  }
  /* 치맛단의 흰 꽃(사이드바와 펼침 메뉴의 아래쪽). */
  .side::after,
  .drawer-box::after {
    content: '';
    position: absolute;
    inset: auto 0 0 0;
    z-index: -1;
    height: 12rem;
    background: var(--petal);
    opacity: var(--petal-alpha);
    -webkit-mask: url('./motif/hem.svg') center bottom / cover no-repeat;
    mask: url('./motif/hem.svg') center bottom / cover no-repeat;
    pointer-events: none;
  }
  .drawer-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--s3);
  }
  @media (min-width: 64rem) {
    .shell {
      grid-template-columns: var(--side-w) minmax(0, 1fr);
    }
    .side {
      position: sticky;
      top: 0;
      isolation: isolate;
      display: flex;
      height: 100dvh;
      border-inline-end: 1px solid var(--border);
      overflow: hidden;
    }
    .side-scroll {
      display: flex;
      flex-direction: column;
      gap: var(--s4);
      width: 100%;
      padding: var(--s5) var(--s3) var(--s3);
      overflow-y: auto;
    }
    .side .brand {
      padding: 0 var(--s3);
    }
    .top,
    .bottom {
      display: none;
    }
    main {
      padding: var(--s6) var(--s6) var(--s7);
    }
  }
  /* 강제 색 모드에서는 배경색이 사라진다. 지금 화면의 표시가 남게 테두리를 두르고, 장식은 그리지 않는다. */
  @media (forced-colors: active) {
    .link[aria-current='page'],
    .bottom a[aria-current='page'] {
      outline: 2px solid Highlight;
      outline-offset: -2px;
    }
    .side::after,
    .drawer-box::after,
    .bud {
      display: none;
    }
  }
  @media (pointer: coarse) {
    .close {
      width: 2.75rem;
      height: 2.75rem;
    }
  }
  @media print {
    .side,
    .top,
    .bottom,
    .skip {
      display: none;
    }
    .column {
      background: none;
    }
  }
</style>
