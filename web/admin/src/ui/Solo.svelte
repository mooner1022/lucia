<script lang="ts">
  import type { Snippet } from 'svelte';
  import { siteName } from '../lib/app.svelte';
  import Icon from './Icon.svelte';
  import Petals from './Petals.svelte';

  // 로그인 없이 여는 화면의 틀(로그인, 초기 설정, 일회용 입력 링크): 가운데 놓인 카드 하나.
  let { title, tone, children }: { title: string; tone?: 'ok' | 'bad'; children?: Snippet } = $props();

  $effect(() => {
    document.title = `${title} · ${siteName()}`;
  });
</script>

<main id="main">
  <Petals fill />
  <div class="card">
    <div class="brand">
      <span class="mark"><Icon name="blossom" size="xl" /></span>
      <span class="name">{siteName()}</span>
      <span class="motto">구름처럼, 조용히 흐르고<br />달빛처럼, 은은히 비추며…</span>
    </div>
    <h1 class={tone ?? ''}>{title}</h1>
    {@render children?.()}
  </div>
</main>

<style>
  /* 하늘에서 치맛단까지: 위는 하늘 번짐, 아래는 푸른 번짐과 흰 꽃. 그 사이로 꽃잎이 흩날린다(Petals). */
  main {
    position: relative;
    isolation: isolate;
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 100dvh;
    padding: var(--s5) var(--s4);
    overflow: clip;
    background:
      radial-gradient(36rem 20rem at 85% -5rem, var(--glow), transparent 72%),
      linear-gradient(180deg, var(--sky) 0%, var(--bg) 42%, var(--wash) 74%, var(--hem) 100%);
  }
  main::after {
    content: '';
    position: absolute;
    z-index: -1;
    inset: auto 0 0 0;
    height: min(15rem, 32vh);
    background: var(--petal);
    opacity: var(--petal-alpha);
    -webkit-mask: url('./motif/hem.svg') center bottom / auto 100% repeat-x;
    mask: url('./motif/hem.svg') center bottom / auto 100% repeat-x;
    pointer-events: none;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--s4);
    width: 100%;
    max-width: 26rem;
    padding: var(--s6) var(--s5);
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
    box-shadow: var(--shadow-2);
  }
  .brand {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--s2);
    padding-bottom: var(--s4);
    border-bottom: 1px solid var(--border);
    color: var(--text);
    text-align: center;
  }
  /* 이름 표식: 쪽빛 바탕의 흰 꽃(Shell과 같다). */
  .mark {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 3.25rem;
    height: 3.25rem;
    border-radius: var(--r-full);
    background: linear-gradient(135deg, #2f3f96, #4a60c0);
    color: #fff;
    box-shadow: var(--shadow-band);
  }
  .name {
    max-width: 100%;
    overflow-wrap: anywhere;
    font-family: var(--font-display);
    font-size: var(--fs-2xl);
    font-weight: 700;
    line-height: 1.2;
    letter-spacing: 0.02em;
  }
  .motto {
    color: var(--text-3);
    font-family: var(--font-display);
    font-size: var(--fs-sm);
    letter-spacing: 0.02em;
  }
  h1 {
    font-family: var(--font-display);
    font-size: var(--fs-xl);
    text-align: center;
  }
  h1.ok { color: var(--ok); }
  h1.bad { color: var(--bad); }
  @media (forced-colors: active) {
    main::after {
      display: none;
    }
  }
  @media print {
    main {
      background: none;
    }
    main::after {
      display: none;
    }
  }
</style>
