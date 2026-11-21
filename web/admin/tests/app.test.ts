// 화면의 서버 왕복, 폼 보내기, 링크 가로채기, 떠나기 전 확인(결정 0061) 시험.
// fetch를 가짜로 바꿔 어떤 요청이 나갔는지와 `app` 상태·DOM을 본다. 서버도 모델도 부르지 않는다.
// 부품은 직접 올려서(mount) 본다. 화면(src/pages)은 고쳐지는 중일 수 있어 App이 필요한 시험만 늦게 불러온다.
import { readFileSync, readdirSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRawSnippet, flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { app, hasDirty, isRaw, load, markDirty, navigate, start, submit } from '../src/lib/app.svelte';
import type { PageData, StandaloneData } from '../src/lib/types';
import ActionForm from '../src/ui/ActionForm.svelte';
import Confirm from '../src/ui/Confirm.svelte';
import PostButton from '../src/ui/PostButton.svelte';
import Screen from '../src/ui/Screen.svelte';
import Shell from '../src/ui/Shell.svelte';
import Standalone from '../src/standalone/Standalone.svelte';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const readJson = (path: string) => JSON.parse(readFileSync(join(ROOT, 'fixtures', path), 'utf8'));
const CSRF: string = readJson('sets/healthy.json').csrf;

// ---------- 시험 데이터 ----------

/** 화면 데이터 하나(안내는 비워 둔다). 이름은 fixtures/pages/healthy.json의 name. */
function fixture(name = '/work', patch: Partial<PageData> = {}): { url: string; data: PageData } {
  const shared = readJson('sets/healthy.json');
  const entry = (readJson('pages/healthy.json') as { name: string; url: string; data: object }[]).find((p) => p.name === name);
  if (!entry) throw new Error(`시험 데이터에 없는 화면: ${name}`);
  return { url: entry.url, data: structuredClone({ ...shared, ...entry.data, flash: null, ...patch }) as PageData };
}
const fixturePage = (name = '/work', patch: Partial<PageData> = {}): PageData => fixture(name, patch).data;

const login = (error: string | null = null, external = false): StandaloneData => ({
  kind: 'standalone',
  name: 'Lucia',
  screen: { type: 'login', error, external },
});
const busy = (): StandaloneData => ({ kind: 'standalone', name: 'Lucia', screen: { type: 'site_login_busy', site: 'qz-site' } });

const pageData = () => app.data as PageData;

// ---------- 가짜 서버 ----------

interface Spec {
  status?: number;
  json?: unknown;
  text?: string;
}
type Answer = Spec | Response;
type Route = Answer | ((init: RequestInit | undefined) => Answer | Promise<Answer>);

const fetchMock = vi.fn<(url: string, init?: RequestInit) => Promise<Response>>();
const confirmMock = vi.fn<(message?: string) => boolean>();

const J = (json: unknown, status = 200): Spec => ({ status, json });
const T = (text: string, status: number): Spec => ({ status, text });

function toResponse(answer: Answer): Response {
  if (answer instanceof Response) return answer;
  const isJson = answer.json !== undefined;
  return new Response(isJson ? JSON.stringify(answer.json) : (answer.text ?? ''), {
    status: answer.status ?? 200,
    headers: { 'content-type': isJson ? 'application/json' : 'text/plain; charset=utf-8' },
  });
}

/** "메서드 주소"별 답. 요청 때마다 새 Response를 만든다(본문은 한 번만 읽힌다). 없는 길은 599로 답한다. */
function serve(routes: Record<string, Route>) {
  fetchMock.mockImplementation(async (url, init) => {
    const key = `${init?.method ?? 'GET'} ${url}`;
    const route = routes[key];
    if (route === undefined) return toResponse(T(`no route: ${key}`, 599));
    const answer = typeof route === 'function' ? await route(init) : route;
    return toResponse(answer);
  });
}

function deferred() {
  let resolve!: (answer: Answer) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<Answer>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

interface Call {
  url: string;
  method: string;
  init: RequestInit;
  headers: Record<string, string>;
  body: BodyInit | null | undefined;
}
const calls = (): Call[] =>
  fetchMock.mock.calls.map(([url, init]) => ({
    url: String(url),
    method: init?.method ?? 'GET',
    init: init ?? {},
    headers: (init?.headers ?? {}) as Record<string, string>,
    body: init?.body,
  }));
const posts = () => calls().filter((c) => c.method === 'POST');
/** urlencoded 본문의 값들. */
const params = (call: Call) => new URLSearchParams(String(call.body));
const keys = (call: Call) => [...new Set([...params(call).keys()])].sort();

/** 대기 중인 마이크로태스크와 짧은 시간을 흘려보내고 화면을 맞춘다. */
async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
  flushSync();
}

// ---------- 그리기 ----------

const mounted: ReturnType<typeof mount>[] = [];

/** 부품 하나를 올린다. 시험이 끝나면 치운다. */
function put(component: unknown, props: Record<string, unknown> = {}): HTMLElement {
  const target = document.createElement('div');
  document.body.append(target);
  mounted.push(mount(component as never, { target, props }));
  flushSync();
  return target;
}

const html = (markup: string) => createRawSnippet(() => ({ render: () => markup }));

/** ActionForm 하나. `inner`는 폼 안의 마크업. */
function putForm(inner: string, props: Record<string, unknown> = {}): HTMLFormElement {
  const target = put(ActionForm, { action: '/memory/save', children: html(`<div>${inner}</div>`), ...props });
  return target.querySelector('form')!;
}

/** 로그인 없는 화면. 바뀌는 `app.data`를 그대로 따라 그린다. */
function putStandalone(): HTMLElement {
  return put(Standalone, {
    get data() {
      return app.data as StandaloneData;
    },
  });
}

function typeInto(el: HTMLInputElement | HTMLTextAreaElement, value: string) {
  el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

/** jsdom의 파일 입력에 파일을 넣는다(공개된 길이 없어 jsdom 안쪽 구현의 파일 목록에 직접 넣는다). */
function attachFile(input: HTMLInputElement, file: File) {
  const { implForWrapper } = createRequire(import.meta.url)('jsdom/lib/generated/idl/utils.js') as {
    implForWrapper(wrapper: object): { files: unknown[] };
  };
  implForWrapper(input).files.push(implForWrapper(file));
}

/** 이 주소(경로+질의). */
const here = () => location.pathname + location.search;

/** `location.assign`을 가로챈다(jsdom의 location은 assign을 바꿀 수 없어, 읽기만 따라가는 객체로 전역을 바꾼다). */
function stubAssign() {
  const real = globalThis.location;
  const assign = vi.fn();
  vi.stubGlobal('location', {
    assign,
    get href() {
      return real.href;
    },
    get origin() {
      return real.origin;
    },
    get pathname() {
      return real.pathname;
    },
    get search() {
      return real.search;
    },
    get hash() {
      return real.hash;
    },
  });
  return assign;
}

function resetApp() {
  app.data = null;
  app.flash = null;
  app.loading = false;
  app.error = null;
  app.url = '/';
  app.version = 0;
  history.replaceState(null, '', '/');
}

/** 로그인한 화면을 지금 화면으로 놓는다(주소도 맞춘다). */
function openPage(name = '/work', patch: Partial<PageData> = {}) {
  const { url, data } = fixture(name, patch);
  history.replaceState(null, '', url);
  app.data = data;
  app.url = url;
  return data;
}

beforeAll(async () => {
  // 한 번만 부르는 start()의 첫 load()가 끝나도록 기다린다.
  vi.stubGlobal('fetch', fetchMock);
  serve({ 'GET /': J(fixturePage('/')) });
  start();
  await settle();
  vi.unstubAllGlobals();
  if (!Element.prototype.scrollIntoView) Element.prototype.scrollIntoView = () => {};
});

beforeEach(() => {
  resetApp();
  fetchMock.mockReset();
  serve({});
  confirmMock.mockReset();
  confirmMock.mockReturnValue(true);
  vi.stubGlobal('fetch', fetchMock);
  vi.stubGlobal('confirm', confirmMock);
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
});

afterEach(() => {
  for (const component of mounted.splice(0)) void unmount(component);
  document.body.innerHTML = '';
  hasDirty(); // DOM에서 사라진 폼을 고치던 것에서 뺀다.
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  delete (document as { visibilityState?: unknown }).visibilityState;
});

// =====================================================================================================
// 1. load()
// =====================================================================================================

describe('load: 서버에 지금 화면을 묻는다', () => {
  it('JSON을 바라고 같은 출처 자격으로 지금 경로와 질의를 묻는다', async () => {
    history.replaceState(null, '', '/activity?kind=mail');
    serve({ 'GET /activity?kind=mail': J(fixturePage('activity?mail')) });
    await load();
    expect(calls()).toHaveLength(1);
    const [call] = calls();
    expect(call.url).toBe('/activity?kind=mail');
    expect(call.method).toBe('GET');
    expect(call.headers.Accept).toBe('application/json');
    expect(call.init.credentials).toBe('same-origin');
    expect(app.url).toBe('/activity?kind=mail');
  });

  it('주소의 #조각은 서버에 보내지 않는다', async () => {
    history.replaceState(null, '', '/work?x=1#frag');
    serve({ 'GET /work?x=1': J(fixturePage('/work')) });
    await load();
    expect(calls().map((c) => c.url)).toEqual(['/work?x=1']);
  });

  it('화면 답은 app.data에 들어가고 오류는 비며 화면이 새로 그려진다(version)', async () => {
    app.error = '옛 오류';
    history.replaceState(null, '', '/work');
    serve({ 'GET /work': J(fixturePage('/work', { title: '받은 화면' })) });
    await load();
    expect(app.data?.kind).toBe('page');
    expect(pageData().title).toBe('받은 화면');
    expect(pageData().csrf).toBe(CSRF);
    expect(app.error).toBeNull();
    expect(app.version).toBe(1);
  });

  it('화면 답에 실린 안내는 app.flash가 된다', async () => {
    history.replaceState(null, '', '/work');
    serve({ 'GET /work': J(fixturePage('/work', { flash: { tone: 'ok', text: '저장했습니다' } })) });
    await load();
    expect(app.flash).toEqual({ tone: 'ok', text: '저장했습니다' });
  });

  it('안내 없는 새 화면은 앞의 안내를 치운다', async () => {
    app.flash = { tone: 'ok', text: '옛 안내' };
    history.replaceState(null, '', '/work');
    serve({ 'GET /work': J(fixturePage('/work')) });
    await load();
    expect(app.flash).toBeNull();
  });

  it('keepFlash면 앞의 안내를 그대로 둔다', async () => {
    app.flash = { tone: 'ok', text: '옛 안내' };
    history.replaceState(null, '', '/work');
    serve({ 'GET /work': J(fixturePage('/work')) });
    await load({ keepFlash: true });
    expect(app.flash).toEqual({ tone: 'ok', text: '옛 안내' });
  });

  it('로그인 없는 화면 답도 그대로 받는다', async () => {
    history.replaceState(null, '', '/login');
    serve({ 'GET /login': J(login()) });
    await load();
    expect(app.data?.kind).toBe('standalone');
  });

  it('받는 동안 loading이 켜지고 끝나면 꺼진다', async () => {
    history.replaceState(null, '', '/work');
    const slow = deferred();
    serve({ 'GET /work': () => slow.promise });
    const pending = load();
    expect(app.loading).toBe(true);
    slow.resolve(J(fixturePage('/work')));
    await pending;
    expect(app.loading).toBe(false);
  });

  describe('조용히 받기(quiet)', () => {
    it('quiet는 version을 올리지 않고 일반 load는 올린다', async () => {
      history.replaceState(null, '', '/work');
      serve({ 'GET /work': () => J(fixturePage('/work')) });
      await load();
      expect(app.version).toBe(1);
      await load();
      expect(app.version).toBe(2);
      await load({ quiet: true });
      expect(app.version).toBe(2);
      await load({ quiet: true });
      expect(app.version).toBe(2);
    });

    it('quiet는 진행 표시(loading)도 켜지 않지만 받은 화면은 반영한다', async () => {
      openPage('/work');
      const slow = deferred();
      serve({ 'GET /work': () => slow.promise });
      const pending = load({ quiet: true });
      expect(app.loading).toBe(false);
      slow.resolve(J(fixturePage('/work', { title: '새로 받음' })));
      await pending;
      expect(pageData().title).toBe('새로 받음');
    });

    it('quiet 받기가 서버에 닿지 못하면 조용히 넘어간다(화면도 안내도 그대로)', async () => {
      openPage('/work');
      fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
      await load({ quiet: true });
      expect(pageData().title).toBe('할 일');
      expect(app.flash).toBeNull();
      expect(app.error).toBeNull();
    });
  });

  describe('이동 답({redirect})', () => {
    it('주소를 바꾸고(교체, 기록을 늘리지 않는다) 간 곳의 화면을 받는다', async () => {
      history.replaceState(null, '', '/old');
      const length = history.length;
      serve({
        'GET /old': J({ redirect: '/work', flash: null }),
        'GET /work': J(fixturePage('/work')),
      });
      await load();
      expect(here()).toBe('/work');
      expect(history.length).toBe(length);
      expect(calls().map((c) => c.url)).toEqual(['/old', '/work']);
      expect(pageData().page).toBe('work');
    });

    it('이동 답에 실린 안내는 간 곳에서도 남는다', async () => {
      history.replaceState(null, '', '/old');
      serve({
        'GET /old': J({ redirect: '/work', flash: { tone: 'warn', text: '다른 곳으로 옮겼습니다' } }),
        'GET /work': J(fixturePage('/work')),
      });
      await load();
      expect(app.flash).toEqual({ tone: 'warn', text: '다른 곳으로 옮겼습니다' });
    });

    it('401 {redirect:"/login"}은 로그인 화면에서 끝난다', async () => {
      history.replaceState(null, '', '/work');
      const length = history.length;
      serve({
        'GET /work': J({ redirect: '/login', flash: null }, 401),
        'GET /login': J(login()),
      });
      await load();
      expect(here()).toBe('/login');
      expect(history.length).toBe(length);
      expect(app.data?.kind).toBe('standalone');
      expect((app.data as StandaloneData).screen.type).toBe('login');
      expect(app.error).toBeNull();
      expect(app.loading).toBe(false);
    });

    it('이미 그 주소면 주소는 건드리지 않는다', async () => {
      history.replaceState(null, '', '/work');
      const length = history.length;
      let n = 0;
      serve({
        'GET /work': () => (n++ === 0 ? J({ redirect: '/work', flash: null }) : J(fixturePage('/work'))),
      });
      await load();
      expect(history.length).toBe(length);
      expect(calls()).toHaveLength(2);
      expect(app.data?.kind).toBe('page');
    });

    it('바깥 주소(https://…)면 location.assign으로 가고 더 묻지 않는다', async () => {
      history.replaceState(null, '', '/work');
      const assign = stubAssign();
      serve({ 'GET /work': J({ redirect: 'https://accounts.example.com/auth?x=1', flash: null }) });
      await load();
      expect(assign).toHaveBeenCalledWith('https://accounts.example.com/auth?x=1');
      expect(calls()).toHaveLength(1);
      expect(here()).toBe('/work');
    });
  });

  describe('받지 못했을 때', () => {
    it('서버에 닿지 못하고 화면이 아직 없으면 app.error를 둔다', async () => {
      fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
      await load();
      expect(app.data).toBeNull();
      expect(app.error).toContain('서버에 닿지 못했습니다');
      expect(app.flash).toBeNull();
      expect(app.loading).toBe(false);
    });

    it('서버에 닿지 못해도 화면이 있으면 화면은 두고 나쁜 안내를 띄운다', async () => {
      openPage('/work');
      fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
      await load();
      expect(pageData().title).toBe('할 일');
      expect(app.error).toBeNull();
      expect(app.flash?.tone).toBe('bad');
      expect(app.flash?.text).toContain('서버에 닿지 못했습니다');
      expect(app.loading).toBe(false);
    });

    it('닿지 못한 뒤 다시 받으면 오류가 사라진다', async () => {
      history.replaceState(null, '', '/work');
      fetchMock.mockRejectedValueOnce(new TypeError('Failed to fetch'));
      await load();
      expect(app.error).not.toBeNull();
      serve({ 'GET /work': J(fixturePage('/work')) });
      await load();
      expect(app.error).toBeNull();
      expect(app.data?.kind).toBe('page');
    });

    it('글로 거절하면(500) 화면이 아직 없을 때 그 글이 app.error가 된다', async () => {
      history.replaceState(null, '', '/work');
      serve({ 'GET /work': T('  내부 오류가 났습니다  ', 500) });
      await load();
      expect(app.error).toBe('내부 오류가 났습니다');
      expect(app.data).toBeNull();
    });

    it('글로 거절하면 화면이 있을 때는 화면을 두고 그 글을 나쁜 안내로 띄운다', async () => {
      openPage('/work');
      serve({ 'GET /work': T('잠시 쉬는 중', 503) });
      await load();
      expect(pageData().title).toBe('할 일');
      expect(app.flash).toEqual({ tone: 'bad', text: '잠시 쉬는 중' });
      expect(app.error).toBeNull();
    });
  });

  describe('늦게 온 옛 답', () => {
    it('더 먼저 물은 느린 답이 나중에 와도 새로 물은 답을 덮어쓰지 않는다', async () => {
      history.replaceState(null, '', '/work');
      const slow = deferred();
      const fast = deferred();
      serve({ 'GET /work': () => slow.promise, 'GET /memory': () => fast.promise });
      const first = load();
      history.replaceState(null, '', '/memory');
      const second = load();

      fast.resolve(J(fixturePage('/memory')));
      await second;
      expect(pageData().page).toBe('memory');

      slow.resolve(J(fixturePage('/work')));
      await first;
      expect(pageData().page).toBe('memory');
      expect(app.version).toBe(1);
      expect(app.loading).toBe(false);
    });

    it('옛 답이 이동 답이어도 따라가지 않는다', async () => {
      history.replaceState(null, '', '/work');
      const slow = deferred();
      const fast = deferred();
      serve({ 'GET /work': () => slow.promise, 'GET /memory': () => fast.promise });
      const first = load();
      history.replaceState(null, '', '/memory');
      const second = load();
      fast.resolve(J(fixturePage('/memory')));
      await second;
      slow.resolve(J({ redirect: '/login', flash: null }, 401));
      await first;
      expect(here()).toBe('/memory');
      expect(calls().map((c) => c.url)).toEqual(['/work', '/memory']);
      expect(pageData().page).toBe('memory');
    });

    it('옛 요청이 뒤늦게 실패해도 새 화면에 오류나 안내를 남기지 않는다', async () => {
      history.replaceState(null, '', '/work');
      const slow = deferred();
      const fast = deferred();
      serve({ 'GET /work': () => slow.promise, 'GET /memory': () => fast.promise });
      const first = load();
      history.replaceState(null, '', '/memory');
      const second = load();
      fast.resolve(J(fixturePage('/memory')));
      await second;
      slow.reject(new TypeError('Failed to fetch'));
      await first;
      expect(app.error).toBeNull();
      expect(app.flash).toBeNull();
      expect(pageData().page).toBe('memory');
    });

    it('더 먼저 물은 답이 먼저 와도 마지막에 물은 답이 남는다', async () => {
      history.replaceState(null, '', '/work');
      const a = deferred();
      const b = deferred();
      serve({ 'GET /work': () => a.promise, 'GET /memory': () => b.promise });
      const first = load();
      history.replaceState(null, '', '/memory');
      const second = load();
      a.resolve(J(fixturePage('/work')));
      await first;
      expect(app.data).toBeNull(); // 옛 답은 버려졌다.
      b.resolve(J(fixturePage('/memory')));
      await second;
      expect(pageData().page).toBe('memory');
      expect(app.loading).toBe(false);
    });
  });
});

// =====================================================================================================
// 2. submit()
// =====================================================================================================

describe('submit: 폼 보내기', () => {
  beforeEach(() => {
    openPage('/work', { csrf: 'csrf-from-page' });
  });

  /** 보내면 지금 화면으로 돌아오라는 답과 그 화면. */
  const saved = (flash: unknown = null) => ({
    'POST /memory/save': J({ redirect: '/work', flash }),
    'GET /work': J(fixturePage('/work', { csrf: 'csrf-from-page' })),
  });

  it('폼의 action으로 POST하고 JSON을 바라며 같은 출처 자격을 쓴다', async () => {
    serve(saved());
    putForm('<input name="title" value="a">').requestSubmit();
    await settle();
    expect(posts()).toHaveLength(1);
    const [post] = posts();
    expect(post.url).toBe('/memory/save');
    expect(post.method).toBe('POST');
    expect(post.headers.Accept).toBe('application/json');
    expect(post.init.credentials).toBe('same-origin');
  });

  it('이름 있는 모든 입력이 urlencoded 본문에 들어간다(이름 없는 것과 꺼진 것은 빠진다)', async () => {
    serve(saved());
    const form = putForm(`
      <input name="title" value="제목 &amp; 값=1">
      <textarea name="body">본문입니다</textarea>
      <select name="kind"><option value="a">A</option><option value="b" selected>B</option></select>
      <input type="hidden" name="id" value="7">
      <input type="radio" name="mode" value="x"><input type="radio" name="mode" value="y" checked>
      <input value="이름 없음">
      <input name="off" value="꺼짐" disabled>`);
    form.requestSubmit();
    await settle();
    const [post] = posts();
    expect(post.body).toBeInstanceOf(URLSearchParams);
    expect(Object.fromEntries(params(post))).toEqual({
      title: '제목 & 값=1',
      body: '본문입니다',
      kind: 'b',
      id: '7',
      mode: 'y',
      csrf: 'csrf-from-page',
      back: '/work',
    });
  });

  it('csrf는 화면 데이터의 csrf, back은 화면 데이터의 path가 붙는다(주소가 아니다)', async () => {
    const data = openPage('character/rev-current', { csrf: 'csrf-rev' });
    expect(data.path).toBe('/character');
    expect(here()).toBe('/character/rev/4');
    serve({ 'POST /memory/save': J({ redirect: '/character', flash: null }), 'GET /character': J(fixturePage('character/rev-current')) });
    putForm('<input name="a" value="1">').requestSubmit();
    await settle();
    const [post] = posts();
    expect(params(post).getAll('csrf')).toEqual(['csrf-rev']);
    expect(params(post).getAll('back')).toEqual(['/character']);
  });

  it('폼에 이미 csrf·back이 있으면 두 번 붙이지 않는다(폼의 값이 남는다)', async () => {
    serve(saved());
    putForm(`<input type="hidden" name="csrf" value="from-form"><input type="hidden" name="back" value="/own"><input name="a" value="1">`).requestSubmit();
    await settle();
    const [post] = posts();
    expect(params(post).getAll('csrf')).toEqual(['from-form']);
    expect(params(post).getAll('back')).toEqual(['/own']);
  });

  it('보낸 단추의 이름과 값이 함께 간다', async () => {
    serve(saved());
    const form = putForm(`<input name="n" value="1"><button type="submit" name="op" value="save">저장</button><button type="submit" name="op" value="delete">삭제</button>`);
    form.querySelectorAll('button')[1].click();
    await settle();
    expect(params(posts()[0]).getAll('op')).toEqual(['delete']);
  });

  it('단추가 여럿이면 누른 것만 간다', async () => {
    serve(saved());
    const form = putForm(
      `<button type="submit" name="a" value="1">가</button><button type="submit" name="b" value="2">나</button><button type="submit" name="c" value="3">다</button>`,
    );
    form.querySelectorAll('button')[0].click();
    await settle();
    const first = params(posts()[0]);
    expect(first.get('a')).toBe('1');
    expect(first.has('b')).toBe(false);
    expect(first.has('c')).toBe(false);

    form.querySelectorAll('button')[2].click();
    await settle();
    const second = params(posts()[1]);
    expect(second.get('c')).toBe('3');
    expect(second.has('a')).toBe(false);
    expect(second.has('b')).toBe(false);
  });

  it('이름 없는 단추를 누르거나 단추 없이 보내면 단추의 값은 가지 않는다', async () => {
    serve(saved());
    const form = putForm(`<button type="submit" name="op" value="save">저장</button><button type="submit">이름 없음</button>`);
    form.querySelectorAll('button')[1].click();
    await settle();
    form.requestSubmit();
    await settle();
    expect(posts()).toHaveLength(2);
    expect(params(posts()[0]).has('op')).toBe(false);
    expect(params(posts()[1]).has('op')).toBe(false);
  });

  it('켜지 않은 체크 상자는 없고 켠 것은 값과 함께 간다(값이 없으면 on)', async () => {
    serve(saved());
    putForm(`<input type="checkbox" name="a" value="yes" checked><input type="checkbox" name="b" value="yes"><input type="checkbox" name="c" checked>`).requestSubmit();
    await settle();
    const [post] = posts();
    expect(params(post).getAll('a')).toEqual(['yes']);
    expect(params(post).has('b')).toBe(false);
    expect(params(post).getAll('c')).toEqual(['on']);
  });

  it('여러 값을 가진 이름(체크 상자 여럿)은 모두 간다', async () => {
    serve(saved());
    putForm(`<input type="checkbox" name="tag" value="x" checked><input type="checkbox" name="tag" value="y" checked>`).requestSubmit();
    await settle();
    expect(params(posts()[0]).getAll('tag')).toEqual(['x', 'y']);
  });

  it('multipart 폼은 FormData로 보내고 content-type을 직접 달지 않는다(파일 포함)', async () => {
    serve({ 'POST /memory/save': J({ redirect: '/work', flash: null }), 'GET /work': J(fixturePage('/work')) });
    const form = putForm('<input type="file" name="file"><input name="title" value="t">', { multipart: true });
    expect(form.enctype).toBe('multipart/form-data');
    const file = new File(['{"installed":1}'], 'client.json', { type: 'application/json' });
    attachFile(form.querySelector('input[type=file]')!, file);
    form.requestSubmit();
    await settle();
    const [post] = posts();
    expect(post.body).toBeInstanceOf(FormData);
    const body = post.body as FormData;
    const sent = body.get('file') as File;
    expect(sent.name).toBe('client.json');
    expect(sent.size).toBe(file.size);
    expect(body.get('title')).toBe('t');
    expect(Object.keys(post.headers).map((k) => k.toLowerCase())).not.toContain('content-type');
    expect(post.headers.Accept).toBe('application/json');
    expect(body.getAll('csrf')).toEqual(['csrf-from-page']);
    expect(body.getAll('back')).toEqual(['/work']);
  });

  it('multipart가 아닌 폼은 FormData가 아니라 urlencoded다', async () => {
    serve(saved());
    putForm('<input name="a" value="1">').requestSubmit();
    await settle();
    expect(posts()[0].body).toBeInstanceOf(URLSearchParams);
    expect(posts()[0].body).not.toBeInstanceOf(FormData);
  });

  describe('서버의 답', () => {
    it('{redirect, flash}를 받으면 안내를 띄우고 화면을 다시 받는다', async () => {
      serve(saved({ tone: 'ok', text: '저장했습니다' }));
      putForm('<input name="a" value="1">').requestSubmit();
      await settle();
      expect(calls().map((c) => `${c.method} ${c.url}`)).toEqual(['POST /memory/save', 'GET /work']);
      expect(app.flash).toEqual({ tone: 'ok', text: '저장했습니다' });
      expect(app.version).toBe(1);
      expect(app.loading).toBe(false);
    });

    it('받은 안내는 화면의 안내 칸에도 실제로 보인다', async () => {
      serve(saved({ tone: 'ok', text: '저장했습니다' }));
      const shell = put(Shell, { data: pageData(), children: html('<div>본문</div>') });
      putForm('<input name="a" value="1">').requestSubmit();
      await settle();
      const banner = shell.querySelector('main .flash');
      expect(banner?.textContent).toContain('저장했습니다');
      expect(banner?.getAttribute('role')).toBe('status');
    });

    // `app.flash`의 설명: "한 번 보이고, 다른 화면으로 가면 사라진다". 안내 없는 이동 답은 앞의 안내를 그대로 둔다
    // (`if (body.flash) app.flash = body.flash`와 `load({ keepFlash: true })`), 예: 로그아웃(안내 없음) 뒤 로그인하면 옛 안내가 홈에 뜬다.
    it('안내가 실리지 않은 이동 답이면 앞 작업의 안내는 치워진다', async () => {
      app.flash = { tone: 'ok', text: '앞 작업의 안내' };
      serve(saved(null));
      putForm('<input name="a" value="1">').requestSubmit();
      await settle();
      expect(app.flash).toBeNull();
    });

    it('다른 경로로 가라는 답이면 주소가 바뀌고(기록이 하나 는다) 그 화면을 받는다', async () => {
      const length = history.length;
      serve({
        'POST /memory/save': J({ redirect: '/memory', flash: { tone: 'ok', text: '만들었습니다' } }),
        'GET /memory': J(fixturePage('/memory')),
      });
      putForm('<input name="a" value="1">').requestSubmit();
      await settle();
      expect(here()).toBe('/memory');
      expect(history.length).toBe(length + 1);
      expect(calls().map((c) => `${c.method} ${c.url}`)).toEqual(['POST /memory/save', 'GET /memory']);
      expect(pageData().page).toBe('memory');
      expect(app.flash).toEqual({ tone: 'ok', text: '만들었습니다' });
    });

    it('같은 경로로 돌아오면 기록을 늘리지 않는다', async () => {
      const length = history.length;
      serve(saved());
      putForm('<input name="a" value="1">').requestSubmit();
      await settle();
      expect(history.length).toBe(length);
    });

    it('바깥 주소(https://…)로 가라는 답이면 location.assign을 부르고 더 받지 않는다', async () => {
      const assign = stubAssign();
      serve({ 'POST /memory/save': J({ redirect: 'https://accounts.example.com/o/oauth?x=1&y=2', flash: { tone: 'info', text: '동의 화면으로 갑니다' } }) });
      putForm('<input name="a" value="1">').requestSubmit();
      await settle();
      expect(assign).toHaveBeenCalledTimes(1);
      expect(assign).toHaveBeenCalledWith('https://accounts.example.com/o/oauth?x=1&y=2');
      expect(calls()).toHaveLength(1);
      expect(here()).toBe('/work');
    });

    it('로그인 없는 화면이 답으로 오면 화면을 그것으로 바꾼다(틀린 비밀번호: 401)', async () => {
      app.data = login();
      history.replaceState(null, '', '/login');
      serve({ 'POST /memory/save': J(login('비밀번호가 맞지 않습니다'), 401) });
      putForm('<input name="password" value="x">').requestSubmit();
      await settle();
      expect(app.data?.kind).toBe('standalone');
      expect((app.data as StandaloneData).screen).toMatchObject({ type: 'login', error: '비밀번호가 맞지 않습니다' });
      expect(app.version).toBe(1);
      expect(here()).toBe('/login');
      expect(calls()).toHaveLength(1);
    });

    it('글로 거절하면(403) 화면은 그대로 두고 그 글을 나쁜 안내로 띄운다', async () => {
      serve({ 'POST /memory/save': T('CSRF 토큰이 맞지 않습니다', 403) });
      putForm('<input name="a" value="1">').requestSubmit();
      await settle();
      expect(calls()).toHaveLength(1);
      expect(pageData().title).toBe('할 일');
      expect(app.version).toBe(0);
      expect(app.flash).toEqual({ tone: 'bad', text: 'CSRF 토큰이 맞지 않습니다' });
      expect(app.error).toBeNull();
      expect(app.loading).toBe(false);
    });

    it('글 없이 거절하면 상태 번호로 알린다', async () => {
      serve({ 'POST /memory/save': T('', 400) });
      putForm('<input name="a" value="1">').requestSubmit();
      await settle();
      expect(app.flash?.tone).toBe('bad');
      expect(app.flash?.text).toContain('400');
    });

    it('너무 긴 거절 글은 300자에서 자른다', async () => {
      serve({ 'POST /memory/save': T('가'.repeat(500), 500) });
      putForm('<input name="a" value="1">').requestSubmit();
      await settle();
      expect(app.flash?.text).toBe('가'.repeat(300));
    });

    it('서버에 닿지 못하면 화면은 그대로 두고 나쁜 안내를 띄운다', async () => {
      fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
      putForm('<input name="a" value="1">').requestSubmit();
      await settle();
      expect(app.flash?.tone).toBe('bad');
      expect(app.flash?.text).toContain('보내지 못했습니다');
      expect(pageData().title).toBe('할 일');
      expect(app.loading).toBe(false);
    });

    it('보내는 동안 loading이 켜져 있다', async () => {
      const slow = deferred();
      serve({ 'POST /memory/save': () => slow.promise, 'GET /work': J(fixturePage('/work')) });
      putForm('<input name="a" value="1">').requestSubmit();
      expect(app.loading).toBe(true);
      slow.resolve(J({ redirect: '/work', flash: null }));
      await settle();
      expect(app.loading).toBe(false);
    });
  });

  describe('두 번 누르기', () => {
    it('보내는 중에 다시 보내도 요청은 하나다', async () => {
      const slow = deferred();
      serve({ 'POST /memory/save': () => slow.promise, 'GET /work': J(fixturePage('/work')) });
      const form = putForm('<input name="a" value="1"><button type="submit">저장</button>');
      form.requestSubmit();
      form.requestSubmit();
      form.querySelector('button')!.click();
      expect(posts()).toHaveLength(1);
      flushSync();
      expect(form.getAttribute('aria-busy')).toBe('true');
      slow.resolve(J({ redirect: '/work', flash: null }));
      await settle();
      expect(posts()).toHaveLength(1);
      expect(form.getAttribute('aria-busy')).toBe('false');
    });

    it('끝난 뒤에는 다시 보낼 수 있다', async () => {
      serve(saved());
      const form = putForm('<input name="a" value="1">');
      form.requestSubmit();
      await settle();
      form.requestSubmit();
      await settle();
      expect(posts()).toHaveLength(2);
    });

    it('실패한 뒤에도 다시 보낼 수 있다', async () => {
      fetchMock.mockRejectedValueOnce(new TypeError('Failed to fetch'));
      const form = putForm('<input name="a" value="1">');
      form.requestSubmit();
      await settle();
      serve(saved());
      form.requestSubmit();
      await settle();
      expect(posts()).toHaveLength(2);
    });
  });

  describe('고치던 것 표시', () => {
    it('보내면 그 폼은 고치던 것에서 빠진다', async () => {
      serve(saved());
      const form = putForm('<input name="a" value="1">');
      typeInto(form.querySelector('input')!, '고침');
      expect(hasDirty()).toBe(true);
      form.requestSubmit();
      await settle();
      expect(hasDirty()).toBe(false);
    });

    it('다른 경로로 이동하는 답이어도 빠진다', async () => {
      serve({ 'POST /memory/save': J({ redirect: '/memory', flash: null }), 'GET /memory': J(fixturePage('/memory')) });
      const form = putForm('<input name="a" value="1">');
      typeInto(form.querySelector('input')!, '고침');
      form.requestSubmit();
      await settle();
      expect(hasDirty()).toBe(false);
    });

    it('서버에 닿지 못하면 고치던 것으로 남는다(입력이 아직 저장되지 않았다)', async () => {
      fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
      const form = putForm('<input name="a" value="1">');
      typeInto(form.querySelector('input')!, '고침');
      form.requestSubmit();
      await settle();
      expect(hasDirty()).toBe(true);
    });

    // 서버가 글로 거절하면(400·403) 화면이 다시 그려지지 않아 입력이 폼에 그대로 남는다. 그러니 계속 지킨다.
    it('서버가 글로 거절하면 고치던 것 표시가 남는다(입력이 저장되지 않았다)', async () => {
      serve({ 'POST /memory/save': T('CSRF 토큰이 맞지 않는다', 403) });
      const form = putForm('<input name="a" value="1">');
      typeInto(form.querySelector('input')!, '고침');
      form.requestSubmit();
      await settle();
      expect(hasDirty()).toBe(true);
    });
  });

  describe('submit 함수 직접', () => {
    it('more는 폼의 값을 덮어쓰거나 덧붙인다', async () => {
      serve(saved());
      const form = putForm('<input name="title" value="원래"><input name="keep" value="k">');
      await submit(form, null, { title: '덮어씀', extra: '1' });
      const sent = params(posts()[0]);
      expect(sent.getAll('title')).toEqual(['덮어씀']);
      expect(sent.get('extra')).toBe('1');
      expect(sent.get('keep')).toBe('k');
    });

    it('화면이 로그인 없는 화면이면 csrf와 back을 붙이지 않는다', async () => {
      app.data = login();
      serve({ 'POST /memory/save': J({ redirect: '/', flash: null }), 'GET /': J(fixturePage('/')) });
      const form = putForm('<input name="a" value="1">');
      await submit(form);
      expect(keys(posts()[0])).toEqual(['a']);
    });

    it('화면이 아직 없어도 보낼 수 있다(csrf 없이)', async () => {
      app.data = null;
      serve({ 'POST /memory/save': T('csrf가 필요합니다', 403) });
      const form = putForm('<input name="a" value="1">');
      await submit(form);
      expect(keys(posts()[0])).toEqual(['a']);
      expect(app.flash).toEqual({ tone: 'bad', text: 'csrf가 필요합니다' });
    });
  });

  describe('PostButton(단추 하나짜리 폼)', () => {
    it('숨은 값과 csrf·back을 싣고 action으로 POST한다', async () => {
      serve({ 'POST /automation/run': J({ redirect: '/work', flash: null }), 'GET /work': J(fixturePage('/work')) });
      const root = put(PostButton, { action: '/automation/run', fields: { id: 'r1', mode: 'now' }, children: html('<span>지금 실행</span>') });
      root.querySelector('button')!.click();
      await settle();
      expect(posts()).toHaveLength(1);
      expect(posts()[0].url).toBe('/automation/run');
      expect(Object.fromEntries(params(posts()[0]))).toEqual({ id: 'r1', mode: 'now', csrf: 'csrf-from-page', back: '/work' });
    });
  });
});

// =====================================================================================================
// 3. method="get"
// =====================================================================================================

// =====================================================================================================
// stay: 처리기가 다른 화면으로 돌려보내도 지금 화면에 남는 폼(초기 설정 화면, 결정 0064)
// =====================================================================================================

describe('submit stay: 지금 화면에 남는 폼', () => {
  beforeEach(() => {
    openPage('/start', { csrf: 'csrf-from-page' });
  });

  /** 처리기는 늘 연결 화면으로 돌려보낸다. 지금 화면(/start)은 다시 받는다. */
  const toConnections = (flash: unknown = { tone: 'ok', text: '저장했습니다' }) => ({
    'POST /discord/token': J({ redirect: '/connections', flash }),
    'GET /start': J(fixturePage('/start', { csrf: 'csrf-from-page' })),
    'GET /connections': J(fixturePage('/connections')),
  });

  it('stay면 서버가 연결 화면으로 가라고 해도 지금 화면을 다시 받고 안내는 그대로 보인다', async () => {
    const length = history.length;
    serve(toConnections());
    putForm('<input name="token" value="x">', { action: '/discord/token', stay: true }).requestSubmit();
    await settle();
    expect(calls().map((c) => `${c.method} ${c.url}`)).toEqual(['POST /discord/token', 'GET /start']);
    expect(here()).toBe('/start');
    expect(history.length).toBe(length);
    expect(app.flash).toEqual({ tone: 'ok', text: '저장했습니다' });
    expect(pageData().page).toBe('start');
    expect(app.version).toBe(1);
  });

  it('stay가 아니면 전처럼 서버가 가라는 곳으로 간다', async () => {
    serve(toConnections());
    putForm('<input name="token" value="x">', { action: '/discord/token' }).requestSubmit();
    await settle();
    expect(calls().map((c) => `${c.method} ${c.url}`)).toEqual(['POST /discord/token', 'GET /connections']);
    expect(here()).toBe('/connections');
  });

  it('질의가 붙은 지금 주소도 그대로 남는다', async () => {
    history.replaceState(null, '', '/start?x=1');
    app.url = '/start?x=1';
    serve({
      'POST /discord/token': J({ redirect: '/connections', flash: null }),
      'GET /start?x=1': J(fixturePage('/start')),
    });
    putForm('<input name="token" value="x">', { action: '/discord/token', stay: true }).requestSubmit();
    await settle();
    expect(here()).toBe('/start?x=1');
    expect(calls().at(-1)?.url).toBe('/start?x=1');
  });

  it('서버가 하지 못했다고 답하면(나쁜 안내) 화면을 새로 그리지 않아 친 값이 남는다', async () => {
    serve(toConnections({ tone: 'bad', text: '토큰을 저장하지 않았습니다' }));
    const form = putForm('<input name="token" value="x">', { action: '/discord/token', stay: true });
    typeInto(form.querySelector('input')!, '친 값');
    form.requestSubmit();
    await settle();
    expect(app.flash).toEqual({ tone: 'bad', text: '토큰을 저장하지 않았습니다' });
    expect(app.version).toBe(0); // 다시 그리지 않는다
    expect(form.querySelector('input')!.value).toBe('친 값');
    expect(hasDirty()).toBe(true); // 아직 저장되지 않은 것으로 남는다
    expect(here()).toBe('/start');
  });

  it('바깥 주소(Google 동의 화면)로 가라는 답은 stay여도 따라간다', async () => {
    const assign = stubAssign();
    serve({ 'POST /google/connect': J({ redirect: 'https://accounts.example.com/o/oauth?x=1', flash: null }) });
    putForm('<input name="a" value="1">', { action: '/google/connect', stay: true }).requestSubmit();
    await settle();
    expect(assign).toHaveBeenCalledWith('https://accounts.example.com/o/oauth?x=1');
    expect(calls()).toHaveLength(1);
    expect(here()).toBe('/start');
  });

  it('로그인으로 가라는 답(세션이 끝났다)도 stay여도 따라간다', async () => {
    serve({
      'POST /discord/token': J({ redirect: '/login', flash: null }),
      'GET /login': J(login()),
    });
    putForm('<input name="token" value="x">', { action: '/discord/token', stay: true }).requestSubmit();
    await settle();
    expect(here()).toBe('/login');
    expect(app.data?.kind).toBe('standalone');
  });

  it('주소가 아닌 이상한 이동(//바깥)은 따라가지 않고 지금 화면에 남는다', async () => {
    serve({
      'POST /discord/token': J({ redirect: '//evil.example/x', flash: null }),
      'GET /start': J(fixturePage('/start')),
    });
    putForm('<input name="token" value="x">', { action: '/discord/token', stay: true }).requestSubmit();
    await settle();
    expect(here()).toBe('/start');
    expect(calls().map((c) => c.url)).toEqual(['/discord/token', '/start']);
  });

  it('PostButton과 Confirm도 stay를 이어 준다', async () => {
    serve({
      'POST /google/connect': J({ redirect: '/connections', flash: { tone: 'bad', text: '먼저 JSON을 저장하세요' } }),
      'POST /profile/avatar/remove': J({ redirect: '/connections', flash: { tone: 'ok', text: '지웠습니다' } }),
      'GET /start': J(fixturePage('/start', { csrf: 'csrf-from-page' })),
    });
    const post = put(PostButton, { action: '/google/connect', stay: true, children: html('연결') });
    post.querySelector('form')!.requestSubmit();
    await settle();
    expect(here()).toBe('/start');
    expect(app.flash?.text).toBe('먼저 JSON을 저장하세요');
    const confirmBox = put(Confirm, {
      action: '/profile/avatar/remove',
      label: '지우기',
      question: '지울까요?',
      stay: true,
    });
    confirmBox.querySelector('dialog form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await settle();
    expect(here()).toBe('/start');
    expect(posts().map((c) => c.url)).toEqual(['/google/connect', '/profile/avatar/remove']);
    expect(app.flash?.text).toBe('지웠습니다');
  });

  it('multipart 폼도 stay로 보낼 수 있다(파일 올리기)', async () => {
    serve({
      'POST /google/client/upload': J({ redirect: '/connections', flash: { tone: 'ok', text: '올렸습니다' } }),
      'GET /start': J(fixturePage('/start', { csrf: 'csrf-from-page' })),
    });
    putForm('<input name="note" value="x">', { action: '/google/client/upload', multipart: true, stay: true }).requestSubmit();
    await settle();
    expect(here()).toBe('/start');
    expect(posts()[0].body).toBeInstanceOf(FormData);
    expect(app.flash).toEqual({ tone: 'ok', text: '올렸습니다' });
  });
});

describe('보이는 이름(siteName)', () => {
  it('화면 답에 실린 이름이다. 답을 받기 전에는 기본 이름', async () => {
    const { siteName } = await import('../src/lib/app.svelte');
    expect(siteName()).toBe('Lucia');
    openPage('/work', { name: '달빛' });
    expect(siteName()).toBe('달빛');
    app.data = login();
    expect(siteName()).toBe('Lucia');
    app.data = { ...login(), name: '루시아' };
    expect(siteName()).toBe('루시아');
  });

  it('머리글·맺음·탭 제목이 그 이름을 쓴다(Shell, Page)', async () => {
    const { default: App } = await import('../src/App.svelte');
    openPage('/work', { name: '달빛' });
    const root = put(App);
    expect(root.querySelector('.brand .name')?.textContent).toContain('달빛');
    expect(root.querySelector('.brand .name')?.textContent).toContain('관리');
    expect(root.querySelector('.footer')?.textContent).toContain('달빛 · 관리 페이지');
    expect(root.textContent).not.toContain('Lucia · 관리 페이지');
    expect(document.title).toBe('할 일 · 달빛');
  });

  it('로그인 없는 화면의 카드 머리와 탭 제목도 그 이름을 쓴다(Solo)', async () => {
    app.data = { ...login(), name: '루시아' };
    const root = putStandalone();
    expect(root.querySelector('.brand .name')?.textContent).toBe('루시아');
    expect(document.title).toMatch(/ · 루시아$/);
  });
});

describe('ActionForm method="get": 찾기', () => {
  beforeEach(() => {
    openPage('/work');
  });

  it('POST하지 않고 action?이름=값으로 그 화면으로 간다', async () => {
    const length = history.length;
    serve({ 'GET /memory?q=%EA%B2%80%EC%83%89+%26+%EA%B0%92&scope=all': J(fixturePage('/memory')) });
    const form = putForm(`<input name="q" value="검색 &amp; 값"><select name="scope"><option value="all" selected>전체</option></select><button type="submit">찾기</button>`, {
      action: '/memory',
      method: 'get',
    });
    form.querySelector('button')!.click();
    await settle();
    expect(posts()).toHaveLength(0);
    expect(calls()).toHaveLength(1);
    expect(calls()[0].method).toBe('GET');
    expect(calls()[0].url).toBe('/memory?q=%EA%B2%80%EC%83%89+%26+%EA%B0%92&scope=all');
    expect(calls()[0].headers.Accept).toBe('application/json');
    expect(here()).toBe('/memory?q=%EA%B2%80%EC%83%89+%26+%EA%B0%92&scope=all');
    expect(history.length).toBe(length + 1);
    expect(pageData().page).toBe('memory');
  });

  it('csrf와 back을 붙이지 않는다(주소에 비밀이 남지 않는다)', async () => {
    serve({});
    putForm('<input name="q" value="a">', { action: '/memory', method: 'get' }).requestSubmit();
    await settle();
    expect(calls()[0].url).toBe('/memory?q=a');
    expect(calls()[0].url).not.toContain('csrf');
  });

  it('입력이 없으면 action만으로 간다', async () => {
    serve({ 'GET /memory': J(fixturePage('/memory')) });
    putForm('<span>없음</span>', { action: '/memory', method: 'get' }).requestSubmit();
    await settle();
    expect(calls()[0].url).toBe('/memory');
    expect(here()).toBe('/memory');
  });

  it('보내는 중 표시(busy) 때문에 막히지 않는다: 다시 찾으면 다시 간다', async () => {
    serve({});
    const form = putForm('<input name="q" value="a">', { action: '/memory', method: 'get' });
    form.requestSubmit();
    await settle();
    form.requestSubmit();
    await settle();
    expect(calls()).toHaveLength(2);
  });

  it('찾기 화면으로 가는 것도 고치던 것이 있으면 묻는다', async () => {
    const other = putForm('<input name="a">');
    typeInto(other.querySelector('input')!, '고침');
    confirmMock.mockReturnValue(false);
    putForm('<input name="q" value="a">', { action: '/memory', method: 'get' }).requestSubmit();
    await settle();
    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(calls()).toHaveLength(0);
    expect(here()).toBe('/work');
  });

  it('onsend는 이동 전에 불린다', async () => {
    serve({});
    const onsend = vi.fn();
    putForm('<input name="q" value="a">', { action: '/memory', method: 'get', onsend }).requestSubmit();
    await settle();
    expect(onsend).toHaveBeenCalledTimes(1);
  });
});

// =====================================================================================================
// 4. 링크 가로채기와 popstate
// =====================================================================================================

/** 링크를 눌러 본다. 돌려주는 것은 앱이 기본 동작을 막았는가. 뒤에서 jsdom의 이동 시도를 막는다. */
function clickOn(el: Element, init: MouseEventInit = {}): boolean {
  let prevented = false;
  const last = (event: Event) => {
    prevented = event.defaultPrevented;
    event.preventDefault();
  };
  window.addEventListener('click', last, { once: true });
  el.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, button: 0, ...init }));
  window.removeEventListener('click', last);
  return prevented;
}

function addLink(href: string, attrs: Record<string, string> = {}, inner = '링크'): HTMLAnchorElement {
  const a = document.createElement('a');
  a.setAttribute('href', href);
  for (const [k, v] of Object.entries(attrs)) a.setAttribute(k, v);
  a.innerHTML = inner;
  document.body.append(a);
  return a;
}

describe('링크 가로채기(start)', () => {
  beforeEach(() => {
    openPage('/memory');
    serve({ 'GET /work': J(fixturePage('/work')), 'GET /work?x=1': J(fixturePage('/work')) });
  });

  it('같은 출처 링크는 화면 안 이동이다: 기록이 늘고 JSON으로 그 주소를 묻는다', async () => {
    const length = history.length;
    expect(clickOn(addLink('/work'))).toBe(true);
    await settle();
    expect(here()).toBe('/work');
    expect(history.length).toBe(length + 1);
    expect(calls()).toHaveLength(1);
    expect(calls()[0].url).toBe('/work');
    expect(calls()[0].method).toBe('GET');
    expect(calls()[0].headers.Accept).toBe('application/json');
    expect(pageData().page).toBe('work');
  });

  it('질의가 있는 주소는 질의째 묻는다', async () => {
    clickOn(addLink('/work?x=1'));
    await settle();
    expect(here()).toBe('/work?x=1');
    expect(calls()[0].url).toBe('/work?x=1');
  });

  it('절대 주소여도 같은 출처면 가로챈다', async () => {
    expect(clickOn(addLink(`${location.origin}/work`))).toBe(true);
    await settle();
    expect(here()).toBe('/work');
    expect(calls()[0].url).toBe('/work');
  });

  it('링크 안쪽 요소를 눌러도 가로챈다', async () => {
    const a = addLink('/work', {}, '<span id="inner">안쪽</span>');
    expect(clickOn(a.querySelector('#inner')!)).toBe(true);
    await settle();
    expect(here()).toBe('/work');
  });

  it('다른 화면의 #조각이 붙은 링크는 이동하고 주소에 #조각이 남는다(서버에는 안 보낸다)', async () => {
    clickOn(addLink('/work?x=1#sec'));
    await settle();
    expect(location.pathname + location.search + location.hash).toBe('/work?x=1#sec');
    expect(calls()[0].url).toBe('/work?x=1');
  });

  it('같은 화면 링크(#조각 없음)는 새로 받는다', async () => {
    openPage('/work');
    const length = history.length;
    expect(clickOn(addLink('/work'))).toBe(true);
    await settle();
    expect(calls()).toHaveLength(1);
    expect(history.length).toBe(length);
  });

  it('화면 위의 메뉴 링크(Shell)도 같은 길로 간다', async () => {
    const shell = put(Shell, { data: pageData(), children: html('<div>본문</div>') });
    const link = shell.querySelector<HTMLAnchorElement>('nav a[href="/work"]')!;
    expect(clickOn(link)).toBe(true);
    await settle();
    expect(here()).toBe('/work');
    expect(calls()[0].url).toBe('/work');
  });

  const leftAlone: [string, string, Record<string, string>, MouseEventInit][] = [
    ['ctrl 누름', '/work', {}, { ctrlKey: true }],
    ['meta 누름', '/work', {}, { metaKey: true }],
    ['shift 누름', '/work', {}, { shiftKey: true }],
    ['alt 누름', '/work', {}, { altKey: true }],
    ['가운데 단추', '/work', {}, { button: 1 }],
    ['오른쪽 단추', '/work', {}, { button: 2 }],
    ['target=_blank', '/work', { target: '_blank' }, {}],
    ['download 속성', '/work', { download: '' }, {}],
    ['다른 출처(도메인)', 'https://example.com/work', {}, {}],
    ['다른 출처(포트)', 'http://localhost:3999/work', {}, {}],
    ['mailto', 'mailto:a@example.com', {}, {}],
    ['파일 내려받기(/files/)', '/files/x/download', {}, {}],
    ['표정 파일(/expressions/file/)', '/expressions/file/x', {}, {}],
    ['정적 파일(/assets/)', '/assets/x.js', {}, {}],
    ['원격 화면 그림', '/browser/handover/frame/1', {}, {}],
    ['아이콘', '/favicon.svg', {}, {}],
    ['같은 화면의 #조각', '#details', {}, {}],
    ['같은 화면의 경로 + #조각', '/memory#details', {}, {}],
  ];
  for (const [name, href, attrs, init] of leftAlone) {
    it(`브라우저에 맡긴다: ${name}`, async () => {
      expect(clickOn(addLink(href, attrs), init)).toBe(false);
      await settle();
      expect(calls()).toHaveLength(0);
      expect(here()).toBe('/memory');
    });
  }

  it('이미 다른 처리기가 막은 클릭은 건드리지 않는다', async () => {
    const a = addLink('/work');
    a.addEventListener('click', (e) => e.preventDefault());
    clickOn(a);
    await settle();
    expect(calls()).toHaveLength(0);
    expect(here()).toBe('/memory');
  });

  it('href 없는 <a>는 건드리지 않는다', async () => {
    const a = document.createElement('a');
    a.textContent = '이름표';
    document.body.append(a);
    expect(clickOn(a)).toBe(false);
    await settle();
    expect(calls()).toHaveLength(0);
  });

  it('고치던 것이 있을 때 이동을 거절하면 그 자리에 있는다(기본 이동도 막힌다)', async () => {
    const form = putForm('<input name="a">');
    typeInto(form.querySelector('input')!, '고침');
    confirmMock.mockReturnValue(false);
    expect(clickOn(addLink('/work'))).toBe(true);
    await settle();
    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(here()).toBe('/memory');
    expect(calls()).toHaveLength(0);
  });
});

describe('뒤로 가기·돌아왔을 때', () => {
  it('popstate는 지금 주소의 화면을 다시 받는다', async () => {
    openPage('/work');
    history.replaceState(null, '', '/skills');
    serve({ 'GET /skills': J(fixturePage('/skills')) });
    window.dispatchEvent(new PopStateEvent('popstate'));
    await settle();
    expect(calls()).toHaveLength(1);
    expect(calls()[0].url).toBe('/skills');
    expect(calls()[0].headers.Accept).toBe('application/json');
    expect(pageData().page).toBe('skills');
  });

  it('popstate는 앞의 안내를 치운다', async () => {
    openPage('/work');
    app.flash = { tone: 'ok', text: '옛 안내' };
    history.replaceState(null, '', '/skills');
    serve({ 'GET /skills': J(fixturePage('/skills')) });
    window.dispatchEvent(new PopStateEvent('popstate'));
    await settle();
    expect(app.flash).toBeNull();
  });

  it('브라우저의 history.back()도 그 앞 화면을 받는다', async () => {
    history.pushState(null, '', '/a-page');
    history.pushState(null, '', '/b-page');
    serve({ 'GET /a-page': J(fixturePage('/work')) });
    history.back();
    await vi.waitFor(() => expect(calls().map((c) => c.url)).toEqual(['/a-page']));
    expect(here()).toBe('/a-page');
    history.pushState(null, '', '/'); // 앞쪽 기록을 잘라 내 다음 시험의 history.length 비교가 어긋나지 않게 한다.
  });

  describe('탭으로 돌아왔을 때(visibilitychange)', () => {
    const visibility = (state: 'visible' | 'hidden') => {
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state });
      document.dispatchEvent(new Event('visibilitychange'));
    };

    it('보이게 되면 조용히 새로 받는다(안내와 version은 그대로)', async () => {
      openPage('/work');
      app.flash = { tone: 'ok', text: '앞의 안내' };
      app.version = 5;
      serve({ 'GET /work': J(fixturePage('/work', { title: '새 제목' })) });
      visibility('visible');
      await settle();
      expect(calls()).toHaveLength(1);
      expect(pageData().title).toBe('새 제목');
      expect(app.flash).toEqual({ tone: 'ok', text: '앞의 안내' });
      expect(app.version).toBe(5);
    });

    it.each([
      ['숨겨질 때', () => openPage('/work'), 'hidden' as const],
      ['로그인 없는 화면일 때', () => (app.data = login()), 'visible' as const],
      ['화면이 아직 없을 때', () => {}, 'visible' as const],
      [
        '고치던 것이 있을 때',
        () => {
          openPage('/work');
          typeInto(putForm('<input name="a">').querySelector('input')!, '고침');
        },
        'visible' as const,
      ],
      [
        '받는 중일 때',
        () => {
          openPage('/work');
          app.loading = true;
        },
        'visible' as const,
      ],
    ])('받지 않는다: %s', async (_name, setup, state) => {
      setup();
      visibility(state);
      await settle();
      expect(calls()).toHaveLength(0);
    });
  });
});

describe('navigate', () => {
  beforeEach(() => {
    openPage('/memory');
    serve({ 'GET /work': J(fixturePage('/work')), 'GET /work?x=1': J(fixturePage('/work')), 'GET /memory': J(fixturePage('/memory')) });
  });

  it('주소를 밀어 넣고 화면을 받는다', async () => {
    const length = history.length;
    await navigate('/work');
    expect(here()).toBe('/work');
    expect(history.length).toBe(length + 1);
    expect(calls()[0].url).toBe('/work');
  });

  it('replace면 기록을 늘리지 않는다', async () => {
    const length = history.length;
    await navigate('/work', { replace: true });
    expect(here()).toBe('/work');
    expect(history.length).toBe(length);
  });

  it('같은 주소면 기록을 늘리지 않고 새로 받는다', async () => {
    const length = history.length;
    await navigate('/memory');
    expect(history.length).toBe(length);
    expect(calls()).toHaveLength(1);
  });

  it('상대 주소도 지금 주소를 기준으로 푼다', async () => {
    history.replaceState(null, '', '/skills/qz');
    serve({ 'GET /skills/other': J(fixturePage('/skills')) });
    await navigate('other');
    expect(here()).toBe('/skills/other');
  });

  it('옮기면 안내가 사라진다', async () => {
    app.flash = { tone: 'ok', text: '앞의 안내' };
    await navigate('/work');
    expect(app.flash).toBeNull();
  });

  it('#조각이 있으면 그 자리로 간다(받을 때는 조각을 빼고, 퍼센트 부호는 푼다)', async () => {
    const target = document.createElement('div');
    target.id = '가나';
    document.body.append(target);
    const spy = vi.spyOn(target, 'scrollIntoView').mockImplementation(() => {});
    await navigate('/work?x=1#%EA%B0%80%EB%82%98');
    expect(location.hash).toBe('#%EA%B0%80%EB%82%98');
    expect(calls()[0].url).toBe('/work?x=1');
    expect(spy).toHaveBeenCalledTimes(1);
    expect(window.scrollTo).not.toHaveBeenCalled();
  });

  it('조각이 없으면 맨 위로 가고 본문에 초점을 둔다', async () => {
    const main = document.createElement('main');
    main.id = 'main';
    main.tabIndex = -1;
    document.body.append(main);
    await navigate('/work');
    expect(window.scrollTo).toHaveBeenCalledWith(0, 0);
    expect(document.activeElement).toBe(main);
  });

  it('깨진 퍼센트 부호가 든 #조각이어도 던지지 않는다', async () => {
    await expect(navigate('/work#%zz')).resolves.toBeUndefined();
  });
});

// =====================================================================================================
// 5. 고치던 것 지키기
// =====================================================================================================

describe('고치던 것 지키기', () => {
  beforeEach(() => {
    openPage('/work');
    serve({ 'GET /memory': J(fixturePage('/memory')) });
  });

  const fireBeforeUnload = () => {
    const event = new Event('beforeunload', { cancelable: true });
    window.dispatchEvent(event);
    return event.defaultPrevented;
  };

  it('markDirty/hasDirty: 켜고 끈다', () => {
    const form = document.createElement('form');
    document.body.append(form);
    expect(hasDirty()).toBe(false);
    markDirty(form, true);
    expect(hasDirty()).toBe(true);
    markDirty(form, false);
    expect(hasDirty()).toBe(false);
  });

  it('POST 폼의 입력 칸에 쓰면 고치던 것이 된다', () => {
    const form = putForm('<input name="a">');
    expect(hasDirty()).toBe(false);
    typeInto(form.querySelector('input')!, '고침');
    expect(hasDirty()).toBe(true);
  });

  it('체크 상자를 눌러도 고치던 것이 된다', () => {
    const form = putForm('<input type="checkbox" name="a" value="1">');
    form.querySelector('input')!.click();
    expect(hasDirty()).toBe(true);
  });

  it('고치던 것이 있으면 navigate가 묻고, 거절하면 그대로 있다', async () => {
    typeInto(putForm('<input name="a">').querySelector('input')!, '고침');
    confirmMock.mockReturnValue(false);
    await navigate('/memory');
    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(confirmMock.mock.calls[0][0]).toContain('저장하지 않은');
    expect(here()).toBe('/work');
    expect(calls()).toHaveLength(0);
    expect(hasDirty()).toBe(true);
  });

  it('수락하면 떠나고 더는 묻지 않는다', async () => {
    typeInto(putForm('<input name="a">').querySelector('input')!, '고침');
    confirmMock.mockReturnValue(true);
    await navigate('/memory');
    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(here()).toBe('/memory');
    expect(calls()).toHaveLength(1);
    expect(hasDirty()).toBe(false);
    await navigate('/memory');
    expect(confirmMock).toHaveBeenCalledTimes(1);
  });

  it('고친 것이 없으면 묻지 않는다', async () => {
    putForm('<input name="a">');
    await navigate('/memory');
    expect(confirmMock).not.toHaveBeenCalled();
    expect(here()).toBe('/memory');
  });

  it('GET 폼은 고치던 것이 되지 않는다', () => {
    const form = putForm('<input name="q">', { action: '/memory', method: 'get' });
    typeInto(form.querySelector('input')!, '찾는 말');
    expect(hasDirty()).toBe(false);
  });

  it('track={false} 폼은 고치던 것이 되지 않는다', () => {
    const form = putForm('<input name="a">', { track: false });
    typeInto(form.querySelector('input')!, '고침');
    expect(hasDirty()).toBe(false);
  });

  it('단추 하나짜리(PostButton)와 묻는 창(Confirm) 안의 입력도 되지 않는다', () => {
    const post = put(PostButton, { action: '/x', children: html('<span>실행</span>') });
    post.querySelector('form')!.dispatchEvent(new Event('input', { bubbles: true }));
    const confirm = put(Confirm, { action: '/y', label: '지우기', question: '지울까요?', children: html('<input name="memo">') });
    typeInto(confirm.querySelector('input[name=memo]') as HTMLInputElement, '메모');
    expect(hasDirty()).toBe(false);
  });

  it('beforeunload는 고치던 것이 있을 때만 막는다', () => {
    expect(fireBeforeUnload()).toBe(false);
    const form = putForm('<input name="a">');
    typeInto(form.querySelector('input')!, '고침');
    expect(fireBeforeUnload()).toBe(true);
    markDirty(form, false);
    expect(fireBeforeUnload()).toBe(false);
  });

  it('DOM에서 사라진 폼은 더 세지 않는다', () => {
    const form = putForm('<input name="a">');
    typeInto(form.querySelector('input')!, '고침');
    expect(hasDirty()).toBe(true);
    form.remove();
    expect(hasDirty()).toBe(false);
    expect(fireBeforeUnload()).toBe(false);
  });

  it('화면이 새로 그려져 폼이 없어지면 묻지 않고 옮긴다', async () => {
    const root = put(ActionForm, { action: '/memory/save', children: html('<div><input name="a"></div>') });
    typeInto(root.querySelector('input')!, '고침');
    root.remove();
    await navigate('/memory');
    expect(confirmMock).not.toHaveBeenCalled();
    expect(here()).toBe('/memory');
  });

  it('폼이 둘이면 하나만 고쳐도 묻고, 하나를 보내도 다른 것이 남는다', async () => {
    serve({ 'POST /memory/save': J({ redirect: '/work', flash: null }), 'GET /work': J(fixturePage('/work')) });
    const a = putForm('<input name="a">');
    const b = putForm('<input name="b">');
    typeInto(a.querySelector('input')!, '1');
    typeInto(b.querySelector('input')!, '2');
    a.requestSubmit();
    await settle();
    expect(hasDirty()).toBe(true);
    markDirty(b, false);
    expect(hasDirty()).toBe(false);
  });
});

// =====================================================================================================
// 6. Confirm
// =====================================================================================================

describe('Confirm: 묻고 나서 보내는 동작', () => {
  beforeEach(() => {
    openPage('/work');
    serve({ 'POST /work/delete': J({ redirect: '/work', flash: { tone: 'ok', text: '지웠습니다' } }), 'GET /work': J(fixturePage('/work')) });
  });

  function putConfirm(props: Record<string, unknown> = {}) {
    const root = put(Confirm, {
      action: '/work/delete',
      label: '삭제',
      question: '정말 지울까요?',
      confirmLabel: '지우기',
      fields: { id: '7', kind: 'task' },
      ...props,
    });
    const dialog = root.querySelector('dialog')!;
    const opener = root.querySelector<HTMLButtonElement>(':scope > button')!;
    return { root, dialog, opener };
  }

  const buttonIn = (dialog: HTMLElement, label: string) =>
    [...dialog.querySelectorAll('button')].find((b) => b.textContent?.trim() === label)!;

  it('처음에는 창이 닫혀 있고, 첫 단추는 창을 열 뿐 아무것도 보내지 않는다', async () => {
    const { dialog, opener } = putConfirm();
    expect(dialog.hasAttribute('open')).toBe(false);
    expect(opener.textContent).toContain('삭제');
    expect(opener.getAttribute('aria-haspopup')).toBe('dialog');
    opener.click();
    await settle();
    expect(dialog.hasAttribute('open')).toBe(true);
    expect(dialog.textContent).toContain('정말 지울까요?');
    expect(calls()).toHaveLength(0);
  });

  it('showModal이 있으면 그것으로 연다', () => {
    const { dialog, opener } = putConfirm();
    const showModal = vi.fn();
    Object.defineProperty(dialog, 'showModal', { configurable: true, value: showModal });
    opener.click();
    expect(showModal).toHaveBeenCalledTimes(1);
  });

  it('showModal이 없으면(jsdom 같은 곳) open 속성으로 열고 닫는다', () => {
    const { dialog, opener } = putConfirm();
    Object.defineProperty(dialog, 'showModal', { configurable: true, value: undefined });
    Object.defineProperty(dialog, 'close', { configurable: true, value: undefined });
    opener.click();
    expect(dialog.hasAttribute('open')).toBe(true);
    buttonIn(dialog, '닫기').click();
    expect(dialog.hasAttribute('open')).toBe(false);
  });

  it('창 안의 폼은 given action으로, 숨은 값과 csrf·back을 싣고 POST한다', async () => {
    const { dialog, opener } = putConfirm();
    opener.click();
    const form = dialog.querySelector('form')!;
    expect(form.getAttribute('action')).toBe('/work/delete');
    buttonIn(dialog, '지우기').click();
    await settle();
    expect(posts()).toHaveLength(1);
    expect(posts()[0].url).toBe('/work/delete');
    expect(Object.fromEntries(params(posts()[0]))).toEqual({ id: '7', kind: 'task', csrf: CSRF, back: '/work' });
    expect(app.flash).toEqual({ tone: 'ok', text: '지웠습니다' });
  });

  it('보내기 시작하면 창이 닫힌다', async () => {
    const { dialog, opener } = putConfirm();
    opener.click();
    expect(dialog.hasAttribute('open')).toBe(true);
    buttonIn(dialog, '지우기').click();
    expect(dialog.hasAttribute('open')).toBe(false);
    await settle();
  });

  it('confirmLabel을 안 주면 label이 확인 단추의 글이다', () => {
    const { dialog } = putConfirm({ confirmLabel: undefined });
    expect(buttonIn(dialog, '삭제')).toBeTruthy();
    expect(buttonIn(dialog, '삭제').getAttribute('type')).toBe('submit');
  });

  it('닫기 단추는 요청 없이 창만 닫는다', async () => {
    const { dialog, opener } = putConfirm();
    opener.click();
    expect(dialog.hasAttribute('open')).toBe(true);
    const close = buttonIn(dialog, '닫기');
    expect(close.getAttribute('type')).toBe('button');
    close.click();
    await settle();
    expect(dialog.hasAttribute('open')).toBe(false);
    expect(calls()).toHaveLength(0);
  });

  it('창 바깥(배경)을 누르면 닫히고 안쪽을 누르면 열려 있다', () => {
    const { dialog, opener } = putConfirm();
    opener.click();
    dialog.querySelector('.box')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(dialog.hasAttribute('open')).toBe(true);
    dialog.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(dialog.hasAttribute('open')).toBe(false);
  });

  it('창 안의 입력(children)도 함께 간다', async () => {
    const { dialog, opener } = putConfirm({ children: html('<input name="reason" value="이유">') });
    opener.click();
    buttonIn(dialog, '지우기').click();
    await settle();
    expect(params(posts()[0]).get('reason')).toBe('이유');
  });
});

// =====================================================================================================
// 7. Screen(원격 화면)
// =====================================================================================================

describe('Screen: 그림을 눌러 좌표 보내기', () => {
  beforeEach(() => {
    openPage('/browser');
    serve({ 'POST /browser/handover/click': J({ redirect: '/browser', flash: null }), 'GET /browser': J(fixturePage('/browser')) });
  });

  function putScreen(box = { left: 10, top: 20, width: 640, height: 360 }) {
    const root = put(Screen, {
      action: '/browser/handover/click',
      src: '/browser/handover/frame/tab-1?t=1',
      alt: '원격 화면',
      fields: { tab: 'qz-tab-one', token: 'tk' },
    });
    const image = root.querySelector<HTMLInputElement>('input[type=image]')!;
    image.getBoundingClientRect = () => ({ ...box, right: box.left + box.width, bottom: box.top + box.height, x: box.left, y: box.top, toJSON() {} }) as DOMRect;
    return image;
  }

  /** 그림의 원래 크기를 정한다(받기 전이면 0). */
  function stubNatural(width: number, height: number) {
    class FakeImage {
      src = '';
      naturalWidth = width;
      naturalHeight = height;
    }
    vi.stubGlobal('Image', FakeImage);
  }

  const clickAt = (image: HTMLElement, clientX: number, clientY: number) => {
    // 마우스·손가락으로 누른 것은 detail이 1 이상이다(키보드로 누르면 0).
    const event = new MouseEvent('click', { bubbles: true, cancelable: true, clientX, clientY, detail: 1 });
    image.dispatchEvent(event);
    return event;
  };

  it('키보드로 누른 것(자리가 없다)은 보내지 않는다: 화면 왼쪽 위 (0,0)을 누르지 않게', async () => {
    stubNatural(1280, 720);
    const image = putScreen();
    const event = new MouseEvent('click', { bubbles: true, cancelable: true, detail: 0 });
    image.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    await settle();
    expect(posts()).toHaveLength(0);
  });

  it('보이는 크기를 원래 크기로 맞춰 at.x·at.y로 보낸다(숨은 값과 csrf·back도 함께)', async () => {
    stubNatural(1280, 720); // 640x360으로 보이니 두 배
    const image = putScreen();
    const event = clickAt(image, 330, 200); // (330-10)*2, (200-20)*2
    expect(event.defaultPrevented).toBe(true);
    await settle();
    expect(posts()).toHaveLength(1);
    expect(posts()[0].url).toBe('/browser/handover/click');
    expect(Object.fromEntries(params(posts()[0]))).toEqual({
      tab: 'qz-tab-one',
      token: 'tk',
      'at.x': '640',
      'at.y': '360',
      csrf: CSRF,
      back: '/browser',
    });
    expect(params(posts()[0]).getAll('at.x')).toHaveLength(1);
  });

  it('가로와 세로 비율이 달라도 각각 맞춘다', async () => {
    stubNatural(1920, 360); // 가로 세 배, 세로 그대로
    const image = putScreen();
    clickAt(image, 110, 70);
    await settle();
    expect(params(posts()[0]).get('at.x')).toBe('300'); // 100*3
    expect(params(posts()[0]).get('at.y')).toBe('50');
  });

  it('원래 크기를 끝내 못 재면(3초) 보이는 크기 그대로 보낸다', async () => {
    // 크기를 모른 채 바로 보내면 줄여 보이는 화면에서 엉뚱한 자리를 누르게 된다. 그래서 잴 때까지 기다리고,
    // 끝내 못 재면 누름을 붙잡아 두지 않고 그대로 보낸다.
    vi.useFakeTimers();
    try {
      stubNatural(0, 0);
      const image = putScreen();
      clickAt(image, 110, 70);
      await vi.advanceTimersByTimeAsync(2900);
      expect(posts()).toHaveLength(0);
      await vi.advanceTimersByTimeAsync(200);
      expect(params(posts()[0]).get('at.x')).toBe('100');
      expect(params(posts()[0]).get('at.y')).toBe('50');
    } finally {
      vi.useRealTimers();
    }
  });

  it('소수는 반올림해 보낸다', async () => {
    stubNatural(1000, 1000);
    const image = putScreen({ left: 10, top: 20, width: 300, height: 300 });
    clickAt(image, 12, 25); // x: 2*3.333=6.67→7, y: 5*3.333=16.67→17
    await settle();
    expect(params(posts()[0]).get('at.x')).toBe('7');
    expect(params(posts()[0]).get('at.y')).toBe('17');
  });

  it('그림 밖(왼쪽·위)을 눌러도 좌표는 0 밑으로 내려가지 않는다', async () => {
    stubNatural(1000, 1000);
    const image = putScreen({ left: 10, top: 20, width: 300, height: 300 });
    clickAt(image, 4, 5);
    await settle();
    expect(params(posts()[0]).get('at.x')).toBe('0');
    expect(params(posts()[0]).get('at.y')).toBe('0');
    clickAt(image, 4, 100);
    await settle();
    expect(params(posts()[1]).get('at.x')).toBe('0');
    expect(params(posts()[1]).get('at.y')).toBe('267'); // 80*3.333
    clickAt(image, 100, 5);
    await settle();
    expect(params(posts()[2]).get('at.x')).toBe('300'); // 90*3.333
    expect(params(posts()[2]).get('at.y')).toBe('0');
  });

  it('보이는 크기가 0이면 나누지 않고 그대로 보낸다', async () => {
    stubNatural(1280, 720);
    const image = putScreen({ left: 0, top: 0, width: 0, height: 0 });
    clickAt(image, 5, 6);
    await settle();
    expect(params(posts()[0]).get('at.x')).toBe('5');
    expect(params(posts()[0]).get('at.y')).toBe('6');
  });

  it('보내는 중에 또 눌러도 요청은 하나다', async () => {
    stubNatural(1280, 720);
    const slow = deferred();
    serve({ 'POST /browser/handover/click': () => slow.promise, 'GET /browser': J(fixturePage('/browser')) });
    const image = putScreen();
    clickAt(image, 20, 30);
    clickAt(image, 40, 50);
    await settle();
    expect(posts()).toHaveLength(1);
    flushSync();
    expect(image.closest('form')!.getAttribute('aria-busy')).toBe('true');
    slow.resolve(J({ redirect: '/browser', flash: null }));
    await settle();
    expect(image.closest('form')!.getAttribute('aria-busy')).toBe('false');
    clickAt(image, 60, 70);
    await settle();
    expect(posts()).toHaveLength(2);
  });

  it('폼 자체를 보내려 해도(Enter 등) 기본 이동은 막힌다', () => {
    const image = putScreen();
    const form = image.closest('form')!;
    const event = new Event('submit', { cancelable: true });
    form.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(posts()).toHaveLength(0);
  });
});

// =====================================================================================================
// 8. Shell
// =====================================================================================================

describe('Shell: 로그인한 화면의 틀', () => {
  function putShell(data: PageData = pageData()) {
    return put(Shell, { data, children: html('<div class="slot">본문</div>') });
  }

  beforeEach(() => {
    openPage('/work');
  });

  it('본문을 넣는다', () => {
    expect(putShell().querySelector('main .slot')?.textContent).toBe('본문');
  });

  describe('처리 결과 안내', () => {
    it('나쁜 안내는 role=alert로 글과 함께 보인다', () => {
      app.flash = { tone: 'bad', text: '실패했습니다' };
      const root = putShell();
      const banner = root.querySelector('main .flash')!;
      expect(banner.getAttribute('role')).toBe('alert');
      expect(banner.textContent).toContain('실패했습니다');
      expect(root.querySelectorAll('[role="alert"]')).toHaveLength(1);
    });

    it.each(['ok', 'info', 'warn', 'muted'] as const)('%s 안내는 role=status다', (tone) => {
      app.flash = { tone, text: '알립니다' };
      const root = putShell();
      const banner = root.querySelector('main .flash')!;
      expect(banner.getAttribute('role')).toBe('status');
      expect(banner.textContent).toContain('알립니다');
      expect(root.querySelector('[role="alert"]')).toBeNull();
    });

    it('안내가 없으면 칸이 없다', () => {
      expect(putShell().querySelector('main .flash')).toBeNull();
    });

    it('나중에 안내가 생기면 그 자리에서 나타난다', () => {
      const root = putShell();
      app.flash = { tone: 'ok', text: '늦게 옴' };
      flushSync();
      expect(root.querySelector('main .flash')?.textContent).toContain('늦게 옴');
    });

    it('닫기 단추는 안내를 지운다', () => {
      app.flash = { tone: 'bad', text: '실패했습니다' };
      const root = putShell();
      const close = root.querySelector<HTMLButtonElement>('main .flash button[aria-label="안내 닫기"]')!;
      expect(close.getAttribute('type')).toBe('button');
      close.click();
      flushSync();
      expect(app.flash).toBeNull();
      expect(root.querySelector('main .flash')).toBeNull();
    });

    it('받는 중이면 진행 표시가 보인다', () => {
      app.loading = true;
      expect(putShell().querySelector('[role="progressbar"]')).not.toBeNull();
    });
  });

  describe('로그아웃', () => {
    it('/logout으로 POST하는 폼이다(링크가 아니다)', () => {
      const root = putShell();
      const forms = root.querySelectorAll<HTMLFormElement>('form[action="/logout"]');
      expect(forms.length).toBeGreaterThanOrEqual(1);
      for (const form of forms) {
        expect(form.getAttribute('method')).toBe('post');
        expect(form.querySelector('button[type="submit"]')).not.toBeNull();
      }
      expect(root.querySelector('a[href="/logout"]')).toBeNull();
    });

    it('보내면 csrf·back과 함께 /logout으로 POST하고 로그인 화면으로 간다', async () => {
      serve({ 'POST /logout': J({ redirect: '/login', flash: null }), 'GET /login': J(login()) });
      const root = putShell();
      root.querySelector<HTMLButtonElement>('form[action="/logout"] button[type="submit"]')!.click();
      await settle();
      expect(posts()).toHaveLength(1);
      expect(posts()[0].url).toBe('/logout');
      expect(params(posts()[0]).get('csrf')).toBe(CSRF);
      expect(here()).toBe('/login');
      expect(app.data?.kind).toBe('standalone');
    });

    it('고치던 것으로 세지 않는다', () => {
      const root = putShell();
      root.querySelector('form[action="/logout"]')!.dispatchEvent(new Event('input', { bubbles: true }));
      expect(hasDirty()).toBe(false);
    });
  });

  describe('메뉴', () => {
    it('지금 화면의 메뉴 칸만 aria-current="page"다(모든 메뉴 모음에서)', () => {
      const root = putShell(pageData());
      const current = [...root.querySelectorAll('a[aria-current]')];
      expect(current.length).toBeGreaterThanOrEqual(2); // 옆 막대, 펼침 메뉴, 아래 막대
      for (const a of current) {
        expect(a.getAttribute('aria-current')).toBe('page');
        expect(a.getAttribute('href')).toBe('/work');
      }
      expect(root.querySelector('a[href="/memory"]')!.hasAttribute('aria-current')).toBe(false);
      expect(root.querySelector('a[href="/"]:not(.brand):not(.state)')!.hasAttribute('aria-current')).toBe(false);
    });

    it('설정의 보조 화면(알림)에서는 설정 칸이 켜진다', () => {
      const data = openPage('/settings/notify');
      const root = putShell(data);
      const hrefs = [...root.querySelectorAll('a[aria-current="page"]')].map((a) => a.getAttribute('href'));
      expect(hrefs.length).toBeGreaterThanOrEqual(1);
      expect(new Set(hrefs)).toEqual(new Set(['/settings']));
    });

    it('홈에서는 홈 칸만 켜진다', () => {
      const data = openPage('/');
      const root = putShell(data);
      const current = [...root.querySelectorAll('a[aria-current="page"]')];
      expect(current.length).toBeGreaterThanOrEqual(1);
      for (const a of current) {
        expect(a.getAttribute('href')).toBe('/');
        expect(a.classList.contains('link') || a.closest('nav.bottom') !== null).toBe(true);
      }
    });

    it('승인 대기 건수 배지는 0보다 클 때만 보인다', () => {
      const none = putShell(pageData());
      expect(none.querySelector('.count')).toBeNull();
      expect(none.querySelector('.pip')).toBeNull();
      expect(none.textContent).not.toContain('승인 대기');
    });

    it('승인이 3건 기다리면 활동 칸에 3이 보인다', () => {
      const root = putShell(openPage('/work', { pending: 3 }));
      const badges = [...root.querySelectorAll('.count')];
      expect(badges.length).toBeGreaterThanOrEqual(1);
      for (const badge of badges) {
        expect(badge.textContent).toContain('3');
        expect(badge.closest('a')?.getAttribute('href')).toBe('/activity');
      }
      expect(root.querySelector('.pip')).not.toBeNull();
    });
  });

  describe('로그인 없는 화면에는 틀이 없다', () => {
    const SENTINEL = 'csrf-LEAK-CHECK-9f3a';

    it('로그인 없는 화면은 로그아웃 폼도 csrf 값도 그리지 않는다', () => {
      app.data = login();
      const root = putStandalone();
      expect(root.querySelector('form[action="/logout"]')).toBeNull();
      expect(root.querySelector('input[name="csrf"]')).toBeNull();
      expect(root.innerHTML).not.toMatch(/csrf/i);
      expect(root.textContent).not.toContain('로그아웃');
    });

    for (const file of readdirSync(join(ROOT, 'fixtures/standalone')).filter((f) => f.endsWith('.json'))) {
      it(`로그인 없는 화면 ${file}`, () => {
        app.data = readJson(`standalone/${file}`) as StandaloneData;
        const root = putStandalone();
        expect(root.querySelector('form[action="/logout"]')).toBeNull();
        expect(root.querySelector('nav')).toBeNull();
        expect(root.querySelector('input[name="csrf"]')).toBeNull();
        expect(root.innerHTML).not.toMatch(/csrf/i);
      });
    }

    it('로그인한 화면에서 로그인 화면으로 바뀌면 csrf와 로그아웃이 DOM에서 사라진다(App)', async () => {
      const { default: App } = await import('../src/App.svelte');
      app.data = fixturePage('/work', { csrf: SENTINEL });
      const root = put(App);
      expect(root.querySelector('form[action="/logout"]')).not.toBeNull();
      expect(document.body.innerHTML).not.toContain(SENTINEL); // 쓸 때 붙이고 DOM에는 두지 않는다
      app.data = login();
      flushSync();
      expect(root.querySelector('form[action="/logout"]')).toBeNull();
      expect(root.querySelector('nav')).toBeNull();
      expect(document.body.innerHTML).not.toContain(SENTINEL);
      expect(document.body.innerHTML).not.toMatch(/csrf/i);
    });
  });
});

// =====================================================================================================
// 9. Standalone
// =====================================================================================================

describe('Standalone: 로그인 없는 화면', () => {
  describe('로그인 폼', () => {
    beforeEach(() => {
      history.replaceState(null, '', '/login');
    });

    it('비밀번호만 /login으로 POST한다(외부가 아니면 코드 칸이 없다)', async () => {
      app.data = login(null, false);
      serve({ 'POST /login': J(login('틀림'), 401) });
      const root = putStandalone();
      expect(root.querySelector('input[name="code"]')).toBeNull();
      typeInto(root.querySelector('input[name="password"]')!, 's3cret pass');
      root.querySelector('form')!.requestSubmit();
      await settle();
      expect(posts()).toHaveLength(1);
      expect(posts()[0].url).toBe('/login');
      expect(posts()[0].headers.Accept).toBe('application/json');
      expect(keys(posts()[0])).toEqual(['password']);
      expect(params(posts()[0]).get('password')).toBe('s3cret pass');
    });

    it('외부에서 들어오면 인증 코드도 함께 보낸다', async () => {
      app.data = login(null, true);
      serve({ 'POST /login': J(login('틀림', true), 401) });
      const root = putStandalone();
      typeInto(root.querySelector('input[name="password"]')!, 's3cret pass');
      typeInto(root.querySelector('input[name="code"]')!, '123 456');
      root.querySelector('form')!.requestSubmit();
      await settle();
      expect(keys(posts()[0])).toEqual(['code', 'password']);
      expect(params(posts()[0]).get('code')).toBe('123 456');
    });

    it('로그인하면 홈 화면을 받고 로그인 화면은 사라진다(App)', async () => {
      const { default: App } = await import('../src/App.svelte');
      app.data = login();
      serve({ 'POST /login': J({ redirect: '/', flash: null }), 'GET /': J(fixturePage('/')) });
      const root = put(App);
      typeInto(root.querySelector('input[name="password"]')!, 'pw');
      root.querySelector('form')!.requestSubmit();
      await settle();
      expect(here()).toBe('/');
      expect(app.data?.kind).toBe('page');
      expect(root.querySelector('input[name="password"]')).toBeNull();
      expect(root.querySelector('form[action="/logout"]')).not.toBeNull();
    });

    it('비밀번호가 틀리면(401 + 로그인 화면) 같은 화면에 오류를 보인다(App)', async () => {
      const { default: App } = await import('../src/App.svelte');
      app.data = login();
      serve({ 'POST /login': J(login('비밀번호가 맞지 않습니다'), 401) });
      const root = put(App);
      typeInto(root.querySelector('input[name="password"]')!, '틀림');
      root.querySelector('form')!.requestSubmit();
      await settle();
      expect(here()).toBe('/login');
      expect(calls()).toHaveLength(1);
      expect(root.querySelector('[role="alert"]')?.textContent).toContain('비밀번호가 맞지 않습니다');
      expect(root.querySelector('input[name="password"]')).not.toBeNull();
    });

    it('로그인 화면은 고치던 것으로 세지 않는다', () => {
      app.data = login();
      const root = putStandalone();
      typeInto(root.querySelector('input[name="password"]')!, 'pw');
      expect(hasDirty()).toBe(false);
    });
  });

  describe('로그인하는 중(site_login_busy)', () => {
    beforeEach(() => {
      history.replaceState(null, '', '/site-login/tok');
    });

    const done = (): StandaloneData => ({ kind: 'standalone', screen: { type: 'site_login_done', site: 'qz-site', note: '끝' } });

    it('3초마다 같은 주소를 조용히 다시 묻는다', async () => {
      vi.useFakeTimers();
      app.data = busy();
      serve({ 'GET /site-login/tok': () => J(busy()) });
      putStandalone();
      await vi.advanceTimersByTimeAsync(2999);
      expect(calls()).toHaveLength(0);
      await vi.advanceTimersByTimeAsync(1);
      expect(calls()).toHaveLength(1);
      expect(calls()[0].url).toBe('/site-login/tok');
      expect(calls()[0].headers.Accept).toBe('application/json');
      expect(app.version).toBe(0); // 조용히 받는다
      await vi.advanceTimersByTimeAsync(3000);
      expect(calls()).toHaveLength(2);
      await vi.advanceTimersByTimeAsync(3000);
      expect(calls()).toHaveLength(3);
    });

    it('아직 하는 중이라는 답이면 계속 묻고, 화면이 바뀌면 그친다', async () => {
      vi.useFakeTimers();
      app.data = busy();
      let n = 0;
      serve({ 'GET /site-login/tok': () => (++n < 3 ? J(busy()) : J(done())) });
      const root = putStandalone();
      await vi.advanceTimersByTimeAsync(3000);
      await vi.advanceTimersByTimeAsync(3000);
      expect(calls()).toHaveLength(2);
      expect((app.data as StandaloneData).screen.type).toBe('site_login_busy');
      await vi.advanceTimersByTimeAsync(3000);
      expect(calls()).toHaveLength(3);
      expect((app.data as StandaloneData).screen.type).toBe('site_login_done');
      expect(root.textContent).toContain('로그인했습니다');
      await vi.advanceTimersByTimeAsync(30000);
      expect(calls()).toHaveLength(3);
    });

    it('그리는 것을 치우면 그친다', async () => {
      vi.useFakeTimers();
      app.data = busy();
      serve({ 'GET /site-login/tok': () => J(busy()) });
      const component = mounted.length;
      putStandalone();
      await vi.advanceTimersByTimeAsync(3000);
      expect(calls()).toHaveLength(1);
      void unmount(mounted.splice(component, 1)[0]);
      await vi.advanceTimersByTimeAsync(30000);
      expect(calls()).toHaveLength(1);
    });

    it('다른 로그인 없는 화면(입력 폼)에서는 묻지 않는다', async () => {
      vi.useFakeTimers();
      app.data = login();
      serve({});
      putStandalone();
      await vi.advanceTimersByTimeAsync(30000);
      expect(calls()).toHaveLength(0);
    });

    it('끝나서 로그인한 화면(page)이 오면 틀로 바뀌고 더 묻지 않는다(App)', async () => {
      vi.useFakeTimers();
      const { default: App } = await import('../src/App.svelte');
      app.data = busy();
      serve({ 'GET /site-login/tok': () => J(fixturePage('/')) });
      const root = put(App);
      await vi.advanceTimersByTimeAsync(3000);
      expect(app.data?.kind).toBe('page');
      expect(root.querySelector('main')).not.toBeNull();
      await vi.advanceTimersByTimeAsync(30000);
      expect(calls()).toHaveLength(1);
    });
  });

  describe('그 밖의 입력 화면', () => {
    it('사이트 로그인 입력은 토큰이 든 주소로 아이디·비밀번호·저장 여부를 보낸다', async () => {
      history.replaceState(null, '', '/site-login/tok-1');
      app.data = {
        kind: 'standalone',
        screen: { type: 'site_login_form', site: 'qz-site', origin: 'https://qz.example', token: 'tok-1', minutes: 10, attempts: 2, error: null },
      };
      serve({ 'POST /site-login/tok-1': J({ redirect: '/site-login/tok-1', flash: null }), 'GET /site-login/tok-1': J(busy()) });
      const root = putStandalone();
      typeInto(root.querySelector('input[name="username"]')!, 'me');
      typeInto(root.querySelector('input[name="password"]')!, 'pw');
      root.querySelector('form')!.requestSubmit();
      await settle();
      expect(posts()[0].url).toBe('/site-login/tok-1');
      expect(Object.fromEntries(params(posts()[0]))).toEqual({ username: 'me', password: 'pw', save: '1' });
    });

    it('초기 설정은 숨은 토큰과 새 비밀번호를 /setup으로 보낸다', async () => {
      history.replaceState(null, '', '/setup/tk');
      app.data = { kind: 'standalone', screen: { type: 'setup', token: 'tk', error: null, min_chars: 8 } };
      serve({ 'POST /setup': J({ redirect: '/login', flash: null }), 'GET /login': J(login()) });
      const root = putStandalone();
      typeInto(root.querySelector('input[name="password"]')!, 'longenough1');
      typeInto(root.querySelector('input[name="confirm"]')!, 'longenough1');
      root.querySelector('form')!.requestSubmit();
      await settle();
      expect(posts()[0].url).toBe('/setup');
      expect(Object.fromEntries(params(posts()[0]))).toEqual({ token: 'tk', password: 'longenough1', confirm: 'longenough1' });
    });
  });
});

// =====================================================================================================
// 10. isRaw
// =====================================================================================================

describe('isRaw: 서버가 직접 내주는 주소', () => {
  it.each([
    '/files/x/download',
    '/files/a',
    '/expressions/file/x',
    '/expressions/file/a/b.png',
    '/browser/handover/frame/1',
    '/browser/handover/frame/tab-1.png',
    '/profile/avatar/file/a_0123456789abcdef01234567',
    '/assets/x.js',
    '/assets/index-abc123.css',
    '/favicon.svg',
  ])('%s는 가로채지 않는다', (path) => {
    expect(isRaw(path)).toBe(true);
  });

  it.each([
    '/',
    '/work',
    '/files',
    '/expressions',
    '/expressions/',
    '/expressions/file',
    '/browser',
    '/browser/handover',
    '/browser/handover/frame',
    '/profile',
    '/profile/avatar',
    '/profile/avatar/file',
    '/assets',
    '/memory/files/x',
    '/myfiles/x',
    '/x/assets/y.js',
    '/favicon.ico',
    '/login',
  ])('%s는 화면이다', (path) => {
    expect(isRaw(path)).toBe(false);
  });

  it('질의가 붙어도 경로로 가른다', () => {
    expect(isRaw('/browser/handover/frame/1?ts=5')).toBe(true);
    expect(isRaw('/work?next=/files/x')).toBe(false);
  });
});
