// 화면 상태와 서버 왕복(결정 0061).
//
// 주소를 정하는 것은 서버다. 화면은 지금 주소를 JSON으로 다시 물어서(`Accept: application/json`) 받은 것을 그린다.
// 그래서 어떤 화면을 그릴지, 없는 주소인지, 로그인으로 돌려보낼지는 서버의 답을 그대로 따른다.
// 쓰기 동작은 예전과 같은 주소로 폼을 보내고, 서버는 "어디로 갈지 + 처리 결과 안내"를 JSON으로 답한다.

import { tick } from 'svelte';
import type { Flash, PageData, ServerReply, StandaloneData } from './types';

export const app = $state({
  /** 지금 그리는 화면(서버의 답). 처음 받기 전에는 없다. */
  data: null as PageData | StandaloneData | null,
  /** 처리 결과 안내(한 번 보이고, 다른 화면으로 가면 사라진다). */
  flash: null as Flash | null,
  /** 받는 중인가(진행 표시). */
  loading: false,
  /** 서버에 닿지 못했을 때의 글. */
  error: null as string | null,
  /** 지금 주소(경로 + 질의). */
  url: '/',
  /** 화면을 새로 그릴 때마다 오른다(폼과 펼침이 처음 상태로 돌아간다). 조용히 새로 받을 때는 그대로다. */
  version: 0,
});

/** 고치다 만 폼의 수(0이 아니면 떠나기 전에 묻는다). */
let dirty = new Set<HTMLFormElement>();
let sequence = 0;

export function markDirty(form: HTMLFormElement, on: boolean) {
  if (on) dirty.add(form);
  else dirty.delete(form);
}

export function hasDirty(): boolean {
  for (const f of dirty) if (!f.isConnected) dirty.delete(f);
  return dirty.size > 0;
}

function here(): string {
  return location.pathname + location.search;
}

/** 관리 화면과 앱에 보이는 이름(결정 0064). 서버의 답에 실려 온다. 답을 받기 전이나 못 받았을 때는 기본 이름이다. */
export function siteName(): string {
  return app.data?.name ?? 'Lucia';
}

async function request(url: string, init?: RequestInit): Promise<{ status: number; body: ServerReply | null; text: string }> {
  const response = await fetch(url, {
    credentials: 'same-origin',
    redirect: 'manual',
    ...init,
    headers: { Accept: 'application/json', ...(init?.headers ?? {}) },
  });
  const text = await response.text();
  let body: ServerReply | null = null;
  if ((response.headers.get('content-type') ?? '').includes('application/json')) {
    try {
      body = JSON.parse(text) as ServerReply;
    } catch {
      body = null;
    }
  }
  return { status: response.status, body, text };
}

/**
 * `stay`인 폼의 답이 가라는 곳: 화면 안 이동(`/…`, 로그인이 아닌 것)이면 서버가 어디로 가라고 하든 지금 화면에 남는다.
 * 바깥 주소(Google 동의 화면)와 로그인으로 가라는 답(세션이 끝났다)은 그대로 따른다.
 * 서버가 주는 값이 아니라 화면이 정하는 것이라 열린 리다이렉트가 생기지 않는다.
 */
function destination(to: string, stay: boolean): string {
  if (!stay || /^https?:\/\//i.test(to)) return to;
  if (to === '/login' || to.startsWith('/login?') || to.startsWith('/login#')) return to;
  return here();
}

/** 서버의 답 하나를 화면에 반영한다. 다른 곳으로 가라는 답이면 간다. */
async function apply(
  reply: { status: number; body: ServerReply | null; text: string },
  from: 'load' | 'submit',
  quiet = false,
  stay = false,
) {
  const { status, body, text } = reply;
  if (body && 'redirect' in body) {
    // 안내가 실리지 않은 이동이면 앞의 안내도 치운다(지난 일의 안내가 다음 화면에 남지 않게).
    app.flash = body.flash ?? null;
    const to = destination(body.redirect, stay);
    if (/^https?:\/\//i.test(to)) {
      // 바깥 주소(Google 동의 화면): 브라우저가 직접 간다.
      location.assign(to);
      return;
    }
    if (to !== here()) history[from === 'load' ? 'replaceState' : 'pushState'](null, '', to);
    await load({ keepFlash: true, quiet });
    return;
  }
  if (body && 'kind' in body) {
    app.data = body;
    app.error = null;
    if (!quiet) app.version++;
    if (body.kind === 'page' && body.flash) app.flash = body.flash;
    return;
  }
  // JSON이 아닌 답: 서버가 글로 거절했다(400·403·500). 화면은 그대로 두고 알린다.
  const why = text.trim().slice(0, 300) || `서버가 ${status}로 답했습니다.`;
  if (from === 'load' && !app.data) app.error = why;
  else app.flash = { tone: 'bad', text: why };
}

/** 지금 주소의 화면을 서버에서 받는다. */
export async function load(options: { keepFlash?: boolean; quiet?: boolean } = {}) {
  const mine = ++sequence;
  app.url = here();
  if (!options.keepFlash) app.flash = null;
  if (!options.quiet) app.loading = true;
  try {
    const reply = await request(here());
    if (mine !== sequence) return;
    await apply(reply, 'load', options.quiet);
  } catch {
    if (mine !== sequence) return;
    const why = '서버에 닿지 못했습니다. 연결을 확인하고 다시 시도하세요.';
    if (app.data && !options.quiet) app.flash = { tone: 'bad', text: why };
    else if (!app.data) app.error = why;
  } finally {
    if (mine === sequence) app.loading = false;
  }
}

/** 화면 안 링크로 옮겨 간다. */
export async function navigate(to: string, options: { replace?: boolean } = {}) {
  if (hasDirty() && !confirm('저장하지 않은 변경이 있습니다. 이 화면을 떠날까요?')) return;
  dirty = new Set();
  const url = new URL(to, location.href);
  const target = url.pathname + url.search;
  if (target !== here() || url.hash) {
    history[options.replace ? 'replaceState' : 'pushState'](null, '', target + url.hash);
  }
  await load();
  if (url.hash) {
    let id = url.hash.slice(1);
    try {
      id = decodeURIComponent(id);
    } catch {
      // 깨진 퍼센트 부호: 적힌 그대로 찾는다.
    }
    document.getElementById(id)?.scrollIntoView();
  } else {
    window.scrollTo(0, 0);
    document.getElementById('main')?.focus({ preventScroll: true });
  }
}

/**
 * 폼을 보낸다. 누른 단추의 name/value도 함께 간다(브라우저가 보내는 것과 같다). `more`는 덧붙이거나 바꿀 값이다.
 * `options.stay`면 서버가 화면 안의 다른 곳으로 가라고 해도(예전 처리기는 늘 연결 화면으로 돌려보낸다) 지금 화면에
 * 남고 안내만 가져온다(초기 설정 화면).
 */
export async function submit(
  form: HTMLFormElement,
  submitter?: HTMLElement | null,
  more?: Record<string, string>,
  options: { stay?: boolean } = {},
): Promise<void> {
  const stay = options.stay ?? false;
  const data = submitter ? new FormData(form, submitter) : new FormData(form);
  for (const [key, value] of Object.entries(more ?? {})) data.set(key, value);
  const page = app.data?.kind === 'page' ? app.data : null;
  if (page) {
    if (!data.has('csrf')) data.set('csrf', page.csrf);
    if (!data.has('back')) data.set('back', page.path);
  }
  const action = form.getAttribute('action') ?? here();
  const multipart = form.enctype === 'multipart/form-data';
  let body: BodyInit;
  if (multipart) {
    body = data;
  } else {
    const fields = new URLSearchParams();
    for (const [key, value] of data) if (typeof value === 'string') fields.append(key, value);
    body = fields;
  }
  const mine = ++sequence;
  app.loading = true;
  try {
    const reply = await request(action, { method: 'POST', body });
    // 서버가 "하지 못했다"고 답하고 같은 화면에 남기는 경우(입력이 틀렸다 등): 화면을 새로 그리지 않는다.
    // 새로 그리면 방금 친 값이 서버의 옛 값으로 덮여 사라진다. 고치던 것 표시도 그대로 둔다.
    const refused =
      !!reply.body &&
      'redirect' in reply.body &&
      reply.body.flash?.tone === 'bad' &&
      destination(reply.body.redirect, stay) === here() &&
      form.isConnected;
    // 서버가 받아들였을 때만 "고치다 만 것"에서 뺀다. 글로 거절한 답(400·403)이면 화면과 입력이 그대로 남으므로 계속 지킨다.
    if (reply.body && !refused) markDirty(form, false);
    if (mine !== sequence) {
      // 그사이 다른 화면으로 갔다. 이 답의 이동은 따르지 않지만, 처리 결과는 알린다(서버에서는 이미 처리됐다).
      if (reply.body && 'redirect' in reply.body && reply.body.flash) app.flash = reply.body.flash;
      return;
    }
    const focus = focusKey(form);
    await apply(reply, 'submit', refused, stay);
    await tick();
    restoreFocus(focus);
  } catch {
    app.flash = { tone: 'bad', text: '서버에 닿지 못해 보내지 못했습니다. 다시 시도하세요.' };
  } finally {
    if (mine === sequence) app.loading = false;
  }
}

/** 보내기 전에 초점이 있던 곳(폼의 주소와 칸의 이름). 화면이 다시 그려진 뒤 같은 곳을 찾는 데 쓴다. */
function focusKey(form: HTMLFormElement): { action: string; name: string | null; submit: boolean } | null {
  const active = document.activeElement;
  if (!(active instanceof HTMLElement) || !form.contains(active)) return null;
  return {
    action: form.getAttribute('action') ?? '',
    name: active.getAttribute('name'),
    submit: active.getAttribute('type') === 'submit',
  };
}

/**
 * 폼을 보낸 뒤 화면이 다시 그려지면 초점이 있던 요소가 사라져 초점이 문서 맨 위로 떨어진다(키보드로는 메뉴를
 * 처음부터 다시 지나야 한다). 같은 폼의 같은 칸(없으면 그 폼의 보내는 단추)으로 되돌리고, 그것도 없으면 본문에 둔다.
 */
function restoreFocus(key: { action: string; name: string | null; submit: boolean } | null) {
  if (document.activeElement && document.activeElement !== document.body) return;
  let target: HTMLElement | null = null;
  if (key) {
    for (const form of document.querySelectorAll<HTMLFormElement>('form')) {
      if ((form.getAttribute('action') ?? '') !== key.action) continue;
      const named = key.name && !key.submit ? form.querySelector<HTMLElement>(`[name="${CSS.escape(key.name)}"]:not([type="hidden"])`) : null;
      target = named ?? form.querySelector<HTMLElement>('[type="submit"]');
      // 묻는 창 안의 폼이면(닫혀 있다) 초점을 줄 수 없다.
      if (target && target.closest('dialog:not([open])')) target = null;
      if (target) break;
    }
  }
  (target ?? document.getElementById('main'))?.focus({ preventScroll: true });
}

/** 서버가 직접 내주는 주소(파일, 그림). 화면 전환으로 가로채지 않는다. */
export function isRaw(path: string): boolean {
  return /^\/(files\/|expressions\/file\/|profile\/avatar\/file\/|browser\/handover\/frame\/|assets\/|favicon\.svg)/.test(path);
}

/** 문서 전체의 링크 누르기와 뒤로 가기를 화면 전환으로 잇는다. 한 번만 부른다. */
export function start() {
  document.addEventListener('click', (event) => {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = (event.target as Element | null)?.closest?.('a[href]') as HTMLAnchorElement | null;
    if (!link || link.target || link.hasAttribute('download')) return;
    const url = new URL(link.href, location.href);
    if (url.origin !== location.origin || isRaw(url.pathname)) return;
    const samePage = url.pathname === location.pathname && url.search === location.search;
    if (samePage && url.hash) return; // 화면 안 건너뛰기는 브라우저가 한다.
    event.preventDefault();
    void navigate(url.pathname + url.search + url.hash);
  });
  window.addEventListener('popstate', () => {
    // 화면 안 건너뛰기(#조각만 바뀜)도 이 일을 일으킨다. 같은 화면이니 다시 받지 않고 묻지도 않는다.
    if (here() === app.url) return;
    // 뒤로·앞으로 가기에도 고치다 만 것이 있으면 묻는다. 남기로 하면 주소를 지금 화면으로 되돌린다.
    if (hasDirty() && !confirm('저장하지 않은 변경이 있습니다. 이 화면을 떠날까요?')) {
      history.pushState(null, '', app.url);
      return;
    }
    dirty = new Set();
    void load();
  });
  window.addEventListener('beforeunload', (event) => {
    if (hasDirty()) event.preventDefault();
  });
  // 돌아왔을 때 새로 받는다(고치던 것이 없을 때만).
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible' && app.data?.kind === 'page' && !hasDirty() && !app.loading) {
      void load({ keepFlash: true, quiet: true });
    }
  });
  void load();
}
