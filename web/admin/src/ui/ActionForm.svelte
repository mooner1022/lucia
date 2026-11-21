<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { HTMLFormAttributes } from 'svelte/elements';
  import { markDirty, navigate, submit } from '../lib/app.svelte';

  // 서버로 보내는 폼. 화면의 모든 쓰기 동작은 이 부품을 거친다(csrf와 돌아올 곳은 보낼 때 붙는다).
  // - method="post"(기본): 같은 주소로 보내고, 서버의 답(갈 곳 + 처리 결과)을 따른다.
  // - method="get": 주소의 질의로 바꿔 그 화면으로 간다(찾기).
  // - `inline`: 줄 안에 놓이는 작은 폼. `track=false`: 고치다 만 것으로 세지 않는다(찾기, 단추 하나짜리).
  // - `stay`: 서버가 화면 안의 다른 곳으로 가라고 해도 지금 화면에 남는다(안내는 가져온다). 같은 처리기를 여러 화면에서
  //   부를 때 쓴다(초기 설정 화면이 연결 화면의 처리기를 부르고도 제자리에 있게). 바깥 주소와 로그인 이동은 그대로 따른다.
  type Props = {
    action: string;
    method?: 'post' | 'get';
    multipart?: boolean;
    inline?: boolean;
    track?: boolean;
    stay?: boolean;
    /** 보내기 시작할 때 부른다(묻는 창 닫기 등). */
    onsend?: () => void;
    children?: Snippet;
  } & Omit<HTMLFormAttributes, 'action' | 'method' | 'children' | 'enctype' | 'onsubmit' | 'oninput'>;

  let {
    action,
    method = 'post',
    multipart = false,
    inline = false,
    track = true,
    stay = false,
    onsend,
    children,
    ...rest
  }: Props = $props();
  let busy = $state(false);

  async function onsubmit(event: SubmitEvent) {
    event.preventDefault();
    const form = event.currentTarget as HTMLFormElement;
    if (method === 'get') {
      const query = new URLSearchParams();
      for (const [key, value] of new FormData(form)) if (typeof value === 'string') query.append(key, value);
      const text = query.toString();
      onsend?.();
      await navigate(text ? `${action}${action.includes('?') ? '&' : '?'}${text}` : action);
      return;
    }
    if (busy) return;
    busy = true;
    onsend?.();
    try {
      await submit(form, event.submitter, undefined, { stay });
    } finally {
      busy = false;
    }
  }

  function oninput(event: Event) {
    if (track && method === 'post') markDirty(event.currentTarget as HTMLFormElement, true);
  }
</script>

<form
  {action}
  {method}
  enctype={multipart ? 'multipart/form-data' : undefined}
  class:inline
  aria-busy={busy}
  {onsubmit}
  {oninput}
  {...rest}
>
  {@render children?.()}
</form>

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: var(--s4);
    margin: 0;
  }
  /* 폼에 바로 놓인 단추는 제 너비만 쓴다(가득 채우려면 Button의 block). */
  form > :global(.btn:not(.block)) {
    align-self: flex-start;
  }
  .inline {
    display: inline-flex;
    flex-direction: row;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--s2);
    max-width: 100%;
  }
  form[aria-busy='true'] {
    opacity: 0.7;
    pointer-events: none;
  }
</style>
