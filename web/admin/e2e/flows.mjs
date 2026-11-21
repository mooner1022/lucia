// 관리 화면 조작 흐름 검사: 실제 브라우저가 실제 서버(시험 데이터 서버의 진짜 라우터)에 대해 화면을 "써 본다"(결정 0061).
//
// check.mjs가 화면의 배치를 보는 것과 달리, 여기서는 사람이 하는 조작(로그인, 메뉴 이동, 폼 보내기, 묻는 창, 모바일 메뉴,
// 원격 화면 누르기, 자동 새로고침, 로그아웃 …)이 서버와 제대로 오가는지 본다. 서버는 데몬의 시험 서버다: 로그인·세션·CSRF·
// 쓰기 처리기·보안 머리는 진짜이고, GET의 화면 데이터만 시험 데이터다(저장소는 비어 있다). 그래서 쓰기 동작의 "성공"이
// 화면 데이터를 바꾸지는 않는다. 여기서 보는 것은 요청이 알맞은 처리기에 알맞은 필드로 닿고 그 답의 안내가 화면에 보이는가이다.
//
// 먼저 화면을 빌드하고 시험 데이터 서버를 띄워 둔다. 한 번에 하려면 web/admin/e2e/run.sh --flows 를 쓴다.
//
//   node e2e/flows.mjs [--base http://127.0.0.1:47890] [--only <단계 이름 일부>] [--viewports desktop,phone]
//
// 데스크톱(1280x900)에서 전부, 휴대폰(360x780, 터치)에서 [m] 표시가 붙은 흐름을 다시 본다.
// 단계마다 "통과/실패" 한 줄을 찍고, 실패해도 다음 단계로 간다. 하나라도 실패하면 종료 코드 1, 서버에 닿지 못하면 2.
// 실패 때의 스크린샷은 <out>/flows/ 에 남는다. 환경 변수: LUCIA_E2E_PASSWORD(기본 fixture-password-123), LUCIA_E2E_LIBS, LUCIA_E2E_FONTS.
//
// 주의(이 검사가 일부러 피하는 것): CSP를 우회하는 도구(bypassCSP, addScriptTag, addStyleTag)는 쓰지 않는다. 기대하는 콘솔 오류
// (일부러 부른 401 같은 것)는 단계가 "기대"로 미리 적어 두고, 그 밖의 콘솔 오류·페이지 오류·CSP 위반·다른 출처 요청은 실패로 센다.

import { mkdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { crc32, deflateSync } from 'node:zlib';
import { launchBrowser } from './launch.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const PASSWORD = process.env.LUCIA_E2E_PASSWORD ?? 'fixture-password-123';

// ---------- 인자 ----------
function usage(message) {
  if (message) console.error(`인자 오류: ${message}\n`);
  console.error('사용법: node e2e/flows.mjs [--base URL] [--only 단계이름일부] [--viewports desktop,phone] [--out 디렉터리]');
  process.exit(2);
}

const VIEWPORTS = {
  desktop: { key: 'desktop', label: '데스크톱', viewport: { width: 1280, height: 900 }, mobile: false },
  phone: { key: 'phone', label: '휴대폰', viewport: { width: 360, height: 780 }, mobile: true },
};

function parseArgs(argv) {
  const opts = { base: 'http://127.0.0.1:47890', only: '', viewports: ['desktop', 'phone'], out: join(here, 'out') };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--help' || arg === '-h') usage();
    const value = argv[++i];
    if (value === undefined) usage(`${arg}에 값이 없습니다`);
    switch (arg) {
      case '--base': opts.base = value.replace(/\/$/, ''); break;
      case '--only': opts.only = value; break;
      case '--viewports': opts.viewports = value.split(',').filter(Boolean); break;
      case '--out': opts.out = resolve(value); break;
      default: usage(`알 수 없는 인자 ${arg}`);
    }
  }
  for (const v of opts.viewports) if (!VIEWPORTS[v]) usage(`뷰포트 ${v}는 desktop, phone 중 하나`);
  return opts;
}

// ---------- 바탕 도구 ----------
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

function fail(message) {
  throw new Error(message);
}
function ok(condition, message) {
  if (!condition) fail(message);
}
function eq(actual, expected, what) {
  if (actual !== expected) fail(`${what}: 기대 ${JSON.stringify(expected)}, 실제 ${JSON.stringify(actual)}`);
}
function near(actual, expected, tolerance, what) {
  if (!(Math.abs(actual - expected) <= tolerance)) fail(`${what}: 기대 ${expected}±${tolerance}, 실제 ${actual}`);
}

/** 조건이 참이 될 때까지 기다린다(시간이 지나도 안 되면 what과 함께 실패). */
async function until(check, what, timeout = 5000) {
  const started = Date.now();
  for (;;) {
    let value;
    try {
      value = await check();
    } catch {
      value = false;
    }
    if (value) return value;
    if (Date.now() - started > timeout) fail(`${what}(${timeout}ms 안에 되지 않음)`);
    await sleep(40);
  }
}

/** 글자가 든 작은 PNG(회색 한 색). 서버가 그림으로 받아들일 모양만 갖춘다. */
function makePng(width, height) {
  const chunk = (type, data) => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(type, 'latin1'), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body) >>> 0);
    return Buffer.concat([length, body, crc]);
  };
  const head = Buffer.alloc(13);
  head.writeUInt32BE(width, 0);
  head.writeUInt32BE(height, 4);
  head[8] = 8; // 8비트
  head[9] = 0; // 회색
  const raw = Buffer.alloc((width + 1) * height, 0x90);
  for (let y = 0; y < height; y++) raw[y * (width + 1)] = 0; // 줄마다 거르개 바이트 0
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', head),
    chunk('IDAT', deflateSync(raw)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// ---------- 진단(콘솔·CSP·다른 출처) ----------
// 일부러 4xx를 부르는 단계는 `expect`로 "이 상태 번호가 올 것"을 미리 적는다. 브라우저는 그런 요청마다
// "Failed to load resource … status of 401" 콘솔 오류를 찍으므로, 그 글도 같은 기대로 걸러진다.
function makeDiag(base) {
  const origin = new URL(base).origin;
  const events = [];
  const allowances = [];
  let stepName = '(단계 밖)';

  const matches = (allowance, event) => {
    if (event.kind === 'http' && allowance.statuses?.includes(event.status)) return true;
    if (event.kind === 'console.error') {
      const failed = /Failed to load resource: the server responded with a status of (\d+)/.exec(event.text);
      if (failed && allowance.statuses?.includes(Number(failed[1]))) return true;
      if (allowance.console && allowance.console.test(event.text)) return true;
    }
    return false;
  };
  const record = (kind, text, extra = {}) => {
    const event = { kind, text: String(text).replace(/\s+/g, ' ').slice(0, 300), step: stepName, ...extra };
    const allowance = allowances.find((a) => matches(a, event));
    if (allowance) {
      allowance.seen++;
      return;
    }
    events.push(event);
  };

  return {
    events,
    setStep(name) {
      stepName = name;
    },
    /** 기대를 건다. 돌려받은 것의 `release()`로 거두고, `seen`은 걸러 낸 횟수다. */
    expect(spec) {
      const allowance = { ...spec, seen: 0 };
      allowances.push(allowance);
      allowance.release = () => {
        const at = allowances.indexOf(allowance);
        if (at >= 0) allowances.splice(at, 1);
      };
      return allowance;
    },
    record,
    attach(page, context) {
      page.on('console', (message) => {
        if (message.type() === 'error') record('console.error', message.text());
      });
      page.on('pageerror', (error) => record('pageerror', error?.stack?.split('\n').slice(0, 2).join(' ') ?? error));
      page.on('response', (response) => {
        const url = response.url();
        if (url.startsWith(origin) && response.status() >= 400) record('http', `${response.status()} ${url.slice(origin.length)}`, { status: response.status() });
      });
      page.on('requestfailed', (request) => {
        const why = request.failure()?.errorText ?? '';
        if (request.url().startsWith(origin) && !/ERR_ABORTED/.test(why)) record('requestfailed', `${request.url().slice(origin.length)} ${why}`);
      });
      page.on('request', (request) => {
        const url = request.url();
        if (/^(data|blob|about):/.test(url)) return;
        if (new URL(url).origin !== origin) record('cross-origin', url);
      });
      return context.exposeBinding('__lfCsp', (_source, text) => record('csp', text));
    },
  };
}

// ---------- 맥락(뷰포트 하나) ----------
async function newContext(browser, vp, opts) {
  const context = await browser.newContext({
    viewport: vp.viewport,
    deviceScaleFactor: 1,
    locale: 'ko-KR',
    colorScheme: 'light',
    hasTouch: vp.mobile,
    isMobile: vp.mobile,
  });
  context.setDefaultTimeout(6000);
  const page = await context.newPage();
  const diag = makeDiag(opts.base);
  const ctx = {
    vp,
    opts,
    base: opts.base,
    context,
    page,
    diag,
    /** 지금까지의 요청(시간순). 단계는 `mark = ctx.reqs.length`를 잡아 두고 그 뒤의 것만 본다. */
    reqs: [],
    /** 페이지 안 fetch가 보낸 FormData(파일 부분은 이름·종류·크기만). */
    forms: [],
    /** 기다리는 묻는 창(alert·confirm) 처리: { action: 'accept'|'dismiss', seen: null|{type, message} } */
    dialogs: [],
    unexpectedDialogs: [],
    notes: [],
  };

  await context.addInitScript(() => {
    document.addEventListener('securitypolicyviolation', (e) => {
      window.__lfCsp?.(`${e.violatedDirective}: ${e.blockedURI || 'inline'} (${e.sourceFile || ''}:${e.lineNumber || 0})`);
    });
    // 폼 본문을 읽어 둔다(파일이 든 multipart는 Playwright가 본문을 못 주는 때가 있다). 앱의 동작은 바꾸지 않는다.
    const original = window.fetch;
    window.fetch = function (input, init) {
      try {
        const body = init?.body;
        if (body instanceof FormData) {
          const entries = [];
          for (const [key, value] of body.entries()) {
            entries.push([key, typeof value === 'string' ? value : { file: value.name, type: value.type, size: value.size }]);
          }
          window.__lfForm?.(JSON.stringify({ url: String(input), entries }));
        }
      } catch {
        // 기록하지 못해도 보내기는 그대로 간다.
      }
      return original.apply(this, arguments);
    };
  });
  await diag.attach(page, context);
  await context.exposeBinding('__lfForm', (_source, text) => ctx.forms.push(JSON.parse(text)));

  page.on('request', (request) => {
    const url = new URL(request.url());
    const headers = request.headers();
    ctx.reqs.push({
      method: request.method(),
      path: url.pathname,
      search: url.search,
      type: request.resourceType(),
      accept: headers.accept ?? '',
      contentType: headers['content-type'] ?? '',
      request,
    });
  });

  // 묻는 창: 단계가 미리 걸어 둔 것만 받는다. 페이지를 떠날 때의 beforeunload는 그냥 보낸다.
  page.on('dialog', async (dialog) => {
    if (dialog.type() === 'beforeunload') {
      await dialog.accept().catch(() => {});
      return;
    }
    const wanted = ctx.dialogs.shift();
    if (!wanted) {
      ctx.unexpectedDialogs.push(`${dialog.type()}: ${dialog.message()}`);
      await dialog.dismiss().catch(() => {});
      return;
    }
    wanted.seen = { type: dialog.type(), message: dialog.message() };
    await (wanted.action === 'accept' ? dialog.accept() : dialog.dismiss()).catch(() => {});
  });
  return ctx;
}

/** 다음 묻는 창을 이렇게 처리하겠다고 걸어 둔다. 돌려받은 것의 `seen`이 채워지면 창이 떴던 것이다. */
function expectDialog(ctx, action) {
  const wanted = { action, seen: null };
  ctx.dialogs.push(wanted);
  return wanted;
}

/** API로 로그인한다(--only로 로그인 단계를 건너뛸 때). */
async function apiLogin(ctx) {
  const response = await ctx.context.request.post(`${ctx.base}/login`, {
    form: { password: PASSWORD },
    headers: { Accept: 'application/json' },
    maxRedirects: 0,
  });
  ok(response.ok(), `API 로그인 실패(HTTP ${response.status()})`);
}

// ---------- 화면 도구 ----------
/** 앱이 그릴 것을 다 그릴 때까지(제목이 생기고 진행 막대가 사라질 때). */
async function settled(ctx) {
  await ctx.page.locator('main#main h1').first().waitFor();
  await ctx.page.locator('[role=progressbar]').waitFor({ state: 'detached' }).catch(() => {});
  await sleep(60);
}

/** 주소를 문서째 연다(그 단계의 깨끗한 시작). */
async function open(ctx, path) {
  await ctx.page.goto(ctx.base + path, { waitUntil: 'load' });
  await settled(ctx);
}

const pathOf = (ctx) => {
  const url = new URL(ctx.page.url());
  return url.pathname + url.search;
};
const h1Of = async (ctx) => (await ctx.page.locator('main#main h1').first().innerText()).trim();

/** 문서 이동이 없었음을 요청 기록과 창 표지로 함께 확인한다. */
async function expectNoDocumentLoad(ctx, mark, marker, what) {
  const documents = ctx.reqs.slice(mark).filter((r) => r.type === 'document').map((r) => r.path + r.search);
  eq(documents.length, 0, `${what}: 문서를 다시 받은 요청 ${JSON.stringify(documents)}`);
  eq(await ctx.page.evaluate(() => window.__flowMarker), marker, `${what}: 창 표지가 사라졌다(문서가 새로 열림)`);
}

async function setMarker(ctx) {
  const marker = `m${Math.random().toString(36).slice(2)}`;
  await ctx.page.evaluate((value) => {
    window.__flowMarker = value;
  }, marker);
  return marker;
}

/** 이 세션의 CSRF 토큰(화면이 보내는 값과 같아야 한다). */
async function csrfOf(ctx) {
  const response = await ctx.context.request.get(`${ctx.base}/settings`, { headers: { Accept: 'application/json' } });
  ok(response.ok(), `CSRF를 읽을 화면 데이터를 못 받았다(HTTP ${response.status()})`);
  return (await response.json()).csrf;
}

/** mark 뒤에 이 경로로 간 POST. */
const postsTo = (ctx, mark, path) => ctx.reqs.slice(mark).filter((r) => r.method === 'POST' && r.path === path);

/** POST가 나가길 기다리고, 잠시 더 두어 "정확히 하나"인지 본다. */
async function exactlyOnePost(ctx, mark, path, what = path) {
  await until(() => postsTo(ctx, mark, path).length > 0, `${what}로 간 POST가 없다`);
  await sleep(900);
  const posts = postsTo(ctx, mark, path);
  eq(posts.length, 1, `${what}로 간 POST 수`);
  return posts[0];
}

const bodyOf = (post) => new URLSearchParams(post.request.postData() ?? '');

/** 처리 결과 안내(flash)를 기다려 읽는다. */
async function flashOf(ctx) {
  const box = ctx.page.locator('main#main .flash');
  await box.waitFor({ state: 'visible' });
  return box.evaluate((el) => ({
    role: el.getAttribute('role'),
    classes: [...el.classList],
    text: (el.querySelector('p')?.textContent ?? '').trim(),
  }));
}

/** 안내가 기대한 결·역할·글인가. tone: ok|info|warn|bad. text: 글 일부(문자열) 또는 정규식. */
async function expectFlash(ctx, { tone, text }) {
  const flash = await flashOf(ctx);
  ok(flash.classes.includes(tone), `안내의 결: 기대 ${tone}, 실제 ${flash.classes.join(' ')}(글: ${flash.text})`);
  eq(flash.role, tone === 'bad' ? 'alert' : 'status', `안내의 role(결 ${tone})`);
  const matched = text instanceof RegExp ? text.test(flash.text) : flash.text.includes(text);
  ok(matched, `안내의 글: 기대 ${text}, 실제 "${flash.text}"`);
  return flash;
}

/** 화면 폭에 맞는 메뉴로 이동한다. 휴대폰에서 아래 막대에 없는 화면은 "전체" 메뉴를 연다. */
const QUICK = ['홈', '할 일', '활동', '기억'];
const drawer = (ctx) => ctx.page.locator('dialog[aria-label="메뉴"]');
async function openDrawer(ctx) {
  await ctx.page.locator('nav[aria-label="빠른 메뉴"]').getByRole('button', { name: '전체' }).click();
  await until(() => drawer(ctx).evaluate((d) => d.open), '"전체"를 눌러도 메뉴 창이 열리지 않는다');
}
async function clickNav(ctx, label) {
  const name = new RegExp(`^${label}`);
  if (!ctx.vp.mobile) {
    await ctx.page.locator('nav[aria-label="주 메뉴"]').getByRole('link', { name }).click();
  } else if (QUICK.includes(label)) {
    await ctx.page.locator('nav[aria-label="빠른 메뉴"]').getByRole('link', { name }).click();
  } else {
    await openDrawer(ctx);
    await drawer(ctx).locator('nav[aria-label="주 메뉴(좁은 화면)"]').getByRole('link', { name }).click();
  }
}

/** 메뉴 세 곳(사이드바·아래 막대·펼침 메뉴) 중 지금 화면이라고 표시된 링크들의 주소. */
async function currentLinks(ctx) {
  return ctx.page.evaluate(() => {
    const out = {};
    for (const nav of document.querySelectorAll('nav[aria-label]')) {
      const label = nav.getAttribute('aria-label');
      if (!['주 메뉴', '빠른 메뉴', '주 메뉴(좁은 화면)'].includes(label)) continue;
      out[label] = [...nav.querySelectorAll('[aria-current="page"]')].map((a) => a.getAttribute('href'));
    }
    return out;
  });
}

/** 설정 보조 탭에서 지금 화면이라고 표시된 것의 글(세 설정 화면은 제목이 모두 "설정"이라 탭으로 가른다). */
const tabCurrent = (ctx) => ctx.page.locator('nav[aria-label="설정 구역"] [aria-current="page"]').innerText().then((t) => t.trim());

/** 지금 화면이 href(메뉴 칸의 주소)로 표시되어 있다: 메뉴마다 그 링크만, 많아야 하나. */
async function expectCurrent(ctx, href) {
  const current = await currentLinks(ctx);
  const own = ctx.vp.mobile ? '주 메뉴(좁은 화면)' : '주 메뉴';
  eq(JSON.stringify(current[own]), JSON.stringify([href]), `${own}의 aria-current`);
  if (ctx.vp.mobile) {
    const quick = current['빠른 메뉴'];
    const inQuick = ['/', '/work', '/activity', '/memory'].includes(href);
    eq(JSON.stringify(quick), JSON.stringify(inQuick ? [href] : []), '빠른 메뉴의 aria-current');
  }
}

// ---------- 단계 ----------
const results = [];

async function step(ctx, name, options, run) {
  const label = `[${ctx.vp.label}] ${name}`;
  if (ctx.opts.only && !name.includes(ctx.opts.only)) return;
  if (options.m === false && ctx.vp.mobile) return;
  if (options.phoneOnly && !ctx.vp.mobile) return;
  ctx.diag.setStep(name);
  ctx.unexpectedDialogs.length = 0;
  ctx.dialogs.length = 0;
  ctx.notes.length = 0;
  const allowance = options.expect ? ctx.diag.expect(options.expect) : null;
  let problem = null;
  try {
    await run(allowance);
    if (ctx.unexpectedDialogs.length > 0) fail(`예상하지 못한 묻는 창: ${ctx.unexpectedDialogs.join(' | ')}`);
  } catch (error) {
    problem = String(error?.message ?? error).split('\n').slice(0, 3).join(' / ');
    try {
      const dir = join(ctx.opts.out, 'flows');
      mkdirSync(dir, { recursive: true });
      await ctx.page.screenshot({ path: join(dir, `${ctx.vp.key}-${name.replace(/[^0-9A-Za-z가-힣]+/g, '-')}.png`) });
    } catch {
      // 스크린샷을 못 찍어도 결과는 그대로다.
    }
  } finally {
    await sleep(120); // 브라우저의 콘솔 글은 응답보다 조금 늦게 온다
    allowance?.release();
  }
  results.push({ label, ok: problem === null, problem });
  console.log(`${problem === null ? '통과' : '실패'}  ${label}${problem === null ? '' : ` — ${problem}`}`);
  for (const note of ctx.notes) console.log(`        참고: ${note}`);
}

// ---------- 본 흐름 ----------
const opts = parseArgs(process.argv.slice(2));
const fixtures = JSON.parse(readFileSync(join(here, '../fixtures/index.json'), 'utf8'));
const memoryEntryUrl = fixtures.pages.find((p) => p.set === 'full' && p.name === 'memory/entry')?.url;
ok(memoryEntryUrl, 'fixtures/index.json에 memory/entry가 없습니다');
// 탭 제목은 "<화면 이름> · <보이는 이름>"이다(결정 0064). 시험 서버의 기본 묶음(full)의 이름.
const siteName = JSON.parse(readFileSync(join(here, '../fixtures/sets/full.json'), 'utf8')).name;
ok(siteName, 'fixtures/sets/full.json에 name이 없습니다');

try {
  await fetch(`${opts.base}/login?fixture=login`, { signal: AbortSignal.timeout(4000), redirect: 'manual' });
} catch {
  console.error(
    `서버에 닿지 못했습니다: ${opts.base}\n` +
      '먼저 web/admin/e2e/run.sh --flows 로 한 번에 돌리거나, 시험 데이터 서버를 띄웁니다:\n' +
      '  LUCIA_UI_FIXTURE_ADDR=127.0.0.1:47890 cargo test -p luciad serve_ui_fixtures -- --ignored --nocapture',
  );
  process.exit(2);
}

let launched;
try {
  launched = await launchBrowser();
} catch (error) {
  console.error(error.message);
  process.exit(2);
}
const { browser, notes: browserNotes } = launched;
for (const note of browserNotes) console.error(`안내: ${note}`);
process.on('SIGINT', () => browser.close().finally(() => process.exit(130)));

const startedAt = Date.now();

async function runViewport(vp) {
  const ctx = await newContext(browser, vp, opts);
  const { page } = ctx;
  const loggedIn = { value: false };
  try {
    // ===== 1. 화면에서 로그인 [m] =====
    await step(ctx, '1 로그인(화면에서)', { expect: { statuses: [401] } }, async () => {
      await page.goto(`${ctx.base}/`);
      await page.waitForURL(`${ctx.base}/login`);
      const password = page.getByLabel('관리 비밀번호');
      await password.waitFor();
      eq(await page.locator('nav').count(), 0, '로그인 화면에 메뉴가 있다');
      // 틀린 비밀번호: 오류 안내가 뜨고 로그인 화면에 머문다.
      await password.fill('wrong-password-for-flow-test');
      await page.getByRole('button', { name: '로그인' }).click();
      const alert = page.getByRole('alert').filter({ hasText: '비밀번호가 틀렸습니다' });
      await alert.waitFor();
      eq(new URL(page.url()).pathname, '/login', '틀린 비밀번호 뒤 주소');
      ok(await page.getByLabel('관리 비밀번호').isVisible(), '틀린 비밀번호 뒤 로그인 폼이 사라졌다');
      // 맞는 비밀번호: 홈과 틀(메뉴)이 나온다(시험 서버는 초기 설정 안내를 끈 채 시작해 첫 화면이 홈이다. 8d가 켜고 끈다).
      await page.getByLabel('관리 비밀번호').fill(PASSWORD);
      await page.getByRole('button', { name: '로그인' }).click();
      await page.waitForURL(`${ctx.base}/`);
      await settled(ctx);
      eq(await h1Of(ctx), '홈', '로그인 뒤 제목');
      const shell = ctx.vp.mobile ? 'nav[aria-label="빠른 메뉴"]' : 'nav[aria-label="주 메뉴"]';
      ok(await page.locator(shell).isVisible(), '로그인 뒤 메뉴(틀)가 보이지 않는다');
      // 세션 쿠키 속성.
      const cookie = (await ctx.context.cookies()).find((c) => c.name === 'lucia_admin');
      ok(cookie, '세션 쿠키 lucia_admin이 없다');
      ok(cookie.httpOnly, '세션 쿠키가 HttpOnly가 아니다');
      eq(cookie.sameSite, 'Strict', '세션 쿠키 SameSite');
      ok(!(await page.evaluate(() => document.cookie)).includes('lucia_admin'), '스크립트가 세션 쿠키를 읽을 수 있다');
      loggedIn.value = true;
    });
    if (!loggedIn.value) {
      // 앞 단계가 실패했거나 건너뛰었다: 나머지를 이어 볼 수 있게 API로 로그인해 둔다.
      await apiLogin(ctx);
      await open(ctx, '/');
    }

    // ===== 2. 문서를 다시 받지 않는 이동 [m] =====
    await step(ctx, '2 이동(문서 다시 받기 없이)', {}, async () => {
      await open(ctx, '/');
      const marker = await setMarker(ctx);
      const mark = ctx.reqs.length;
      const seen = [await h1Of(ctx)];
      const visit = async (label, path, href) => {
        await clickNav(ctx, label);
        await until(async () => pathOf(ctx) === path && (await h1Of(ctx)) !== seen.at(-1), `${label}으로 가도 주소·제목이 바뀌지 않는다(주소 ${pathOf(ctx)})`);
        seen.push(await h1Of(ctx));
        await expectCurrent(ctx, href);
        // 화면을 옮기면 초점이 본문(#main)으로 가고 문서 제목도 새 화면을 따른다.
        const focused = () => page.evaluate(() => document.activeElement?.id || document.activeElement?.tagName.toLowerCase());
        await until(async () => (await focused()) === 'main', `${label}으로 간 뒤 초점이 본문(#main)으로 오지 않는다(초점: ${await focused()})`);
        // 설정의 탭들은 h1("설정")을 나눠 쓰고 문서 제목으로 가른다("설정 · 안전"). 그래서 제목은 h1로 시작하는지만 본다.
        ok((await page.title()).startsWith(seen.at(-1)) && (await page.title()).endsWith(` · ${siteName}`), `${label}으로 간 뒤 문서 제목: ${await page.title()}`);
        if (ctx.vp.mobile) ok(!(await drawer(ctx).evaluate((d) => d.open)), `${label}을 고른 뒤 메뉴 창이 닫히지 않았다`);
      };
      await visit('할 일', '/work', '/work');
      await visit('자동화', '/automation', '/automation');
      await visit('설정', '/settings', '/settings');
      const settingsTitle = seen.at(-1);
      // 설정 안의 보조 탭: 메뉴의 "설정" 칸은 그대로 켜져 있다.
      eq(await tabCurrent(ctx), '안전과 한도', '설정 보조 탭의 처음 표시');
      await page.locator('nav[aria-label="설정 구역"]').getByRole('link', { name: '알림' }).click();
      await until(async () => pathOf(ctx) === '/settings/notify' && (await tabCurrent(ctx)) === '알림', '설정 보조 탭 "알림"으로 가지지 않는다');
      await expectCurrent(ctx, '/settings');
      // 뒤로·앞으로.
      await page.goBack();
      await until(async () => pathOf(ctx) === '/settings' && (await tabCurrent(ctx)) === '안전과 한도', '뒤로 가기가 /settings를 그리지 않는다');
      eq(await h1Of(ctx), settingsTitle, '뒤로 간 /settings의 제목');
      await page.goBack();
      // 뒤로 가기는 주소가 먼저 바뀌고 화면은 서버의 답을 받은 뒤에 바뀐다.
      await until(async () => pathOf(ctx) === '/automation' && (await h1Of(ctx)) === seen[2], `뒤로 가기(두 번째)가 /automation을 그리지 않는다(주소 ${pathOf(ctx)}, 제목 ${await h1Of(ctx)})`);
      await page.goForward();
      await until(async () => pathOf(ctx) === '/settings', '앞으로 가기가 /settings로 가지 않는다');
      await page.goForward();
      await until(async () => pathOf(ctx) === '/settings/notify' && (await tabCurrent(ctx)) === '알림', '앞으로 가기(두 번째)가 /settings/notify를 그리지 않는다');
      await expectNoDocumentLoad(ctx, mark, marker, '메뉴 이동·뒤로·앞으로');
      // 주소를 바로 여는 것(깊은 링크).
      await open(ctx, '/settings/notify');
      eq(await tabCurrent(ctx), '알림', '/settings/notify를 바로 열었을 때 설정 보조 탭');
      eq(await h1Of(ctx), settingsTitle, '/settings/notify를 바로 열었을 때 제목');
      const api = await ctx.context.request.get(ctx.base + memoryEntryUrl, { headers: { Accept: 'application/json' } });
      const title = (await api.json()).extra.entry.title;
      await open(ctx, memoryEntryUrl);
      eq(await h1Of(ctx), title, `${memoryEntryUrl}를 바로 열었을 때 제목`);
      eq(pathOf(ctx), memoryEntryUrl, '깊은 링크 뒤 주소');
    });

    // 휴대폰: 펼침 메뉴의 열고 닫기(초점, 닫기 단추, Esc).
    await step(ctx, '2b 휴대폰 펼침 메뉴(열기·초점·닫기)', { phoneOnly: true }, async () => {
      await open(ctx, '/work');
      const state = () =>
        drawer(ctx).evaluate((d) => ({ open: d.open, modal: d.matches(':modal'), focusIn: d.contains(document.activeElement), focus: document.activeElement?.tagName }));
      await openDrawer(ctx);
      let s = await state();
      ok(s.modal, '메뉴 창이 :modal이 아니다');
      ok(s.focusIn, `메뉴 창을 열었는데 초점이 창 밖에 있다(${s.focus})`);
      // 창 뒤의 것은 초점을 받지 못한다.
      const behind = await page.evaluate(() => {
        const link = [...document.querySelectorAll('main a[href], header a[href]')].find((a) => a.getClientRects().length > 0 && !a.closest('dialog'));
        if (!link) return null;
        link.focus();
        return document.activeElement === link;
      });
      ok(behind === false, `메뉴 창 뒤의 링크가 초점을 받았다(${behind})`);
      await page.keyboard.press('Escape');
      await until(async () => !(await state()).open, 'Esc로 메뉴 창이 닫히지 않는다');
      ok(
        await page.evaluate(() => document.activeElement?.tagName === 'BUTTON' && document.activeElement.textContent.includes('전체')),
        'Esc로 닫은 뒤 초점이 "전체" 단추로 돌아오지 않았다',
      );
      // 창 밖(어두운 막)을 누르면 닫힌다.
      await openDrawer(ctx);
      await page.touchscreen.tap(8, 400);
      await until(async () => !(await state()).open, '메뉴 창 밖을 눌러도 닫히지 않는다');
      eq(pathOf(ctx), '/work', '창 밖을 눌렀을 뿐인데 주소가 바뀌었다');
      await openDrawer(ctx);
      await drawer(ctx).getByRole('button', { name: '닫기' }).click();
      await until(async () => !(await state()).open, '"닫기" 단추로 메뉴 창이 닫히지 않는다');
      eq(pathOf(ctx), '/work', '메뉴를 닫았을 뿐인데 주소가 바뀌었다');
      // 위 막대의 "메뉴" 단추도 같은 창을 연다.
      await page.locator('header').getByRole('button', { name: '메뉴' }).click();
      await until(async () => (await state()).open, '위 막대의 "메뉴" 단추가 메뉴 창을 열지 않는다');
      await page.keyboard.press('Escape');
    });

    // ===== 2c·2d. 같은 화면 안 건너뛰기 링크(#구역) =====
    // 화면 안의 구역 링크(Jump)는 브라우저가 스크롤만 한다. 화면을 다시 받거나, 펼침·입력을 날리거나, 떠나겠느냐고 묻지 않아야 한다.
    await step(ctx, '2c 화면 안 건너뛰기 링크는 화면을 다시 받지 않는다', { m: false }, async () => {
      await open(ctx, '/expressions');
      const marker = await setMarker(ctx);
      const fold = page.locator('details', { hasText: 'Lucia에게 맡기기' });
      await fold.locator('summary').click();
      ok(await fold.evaluate((d) => d.open), '펼치기 시험 준비: 펼침이 열리지 않았다');
      const mark = ctx.reqs.length;
      await page.getByRole('link', { name: /^스티커/ }).first().click();
      await until(() => new URL(page.url()).hash === '#stickers', '건너뛰기 링크를 눌러도 주소에 #stickers가 붙지 않는다');
      await sleep(900);
      // 문제를 모아서 한 번에 알린다(다시 받기와 펼침 닫힘은 같은 뿌리의 증상이다).
      const problems = [];
      const refetch = ctx.reqs.slice(mark).filter((r) => r.path === '/expressions');
      if (refetch.length > 0) problems.push(`화면 데이터를 다시 받았다 ${JSON.stringify(refetch.map((r) => `${r.method} ${r.path}`))}`);
      if (!(await fold.evaluate((d) => d.open))) problems.push('펼쳐 둔 것이 닫혔다(화면이 다시 그려졌다)');
      if (problems.length > 0) fail(`구역 링크(#stickers)를 눌렀더니 ${problems.join(', ')}`);
      await expectNoDocumentLoad(ctx, mark, marker, '구역 링크');
    });
    await step(ctx, '2d 화면 안 건너뛰기 링크가 "떠날까요?"를 묻지 않는다', { m: false }, async () => {
      await open(ctx, '/expressions');
      const input = page.locator('form[action="/expressions/add"] input[name=name]');
      await input.fill('typed-text');
      const dismissed = expectDialog(ctx, 'dismiss');
      await page.getByRole('link', { name: /^스티커/ }).first().click();
      await sleep(900);
      ok(!dismissed.seen, `같은 화면 안의 구역 링크가 떠날 때 묻는 창을 띄웠다: "${dismissed.seen?.message}"`);
      eq(new URL(page.url()).hash, '#stickers', '구역 링크를 누른 뒤 주소의 해시');
      eq(await input.inputValue(), 'typed-text', '구역 링크를 눌렀더니 친 값이 사라졌다');
    });

    // ===== 3. 일반 폼 =====
    await step(ctx, '3 일반 폼(자동 실행 한도, /autonomy/limits)', { m: false }, async () => {
      await open(ctx, '/settings');
      const csrf = await csrfOf(ctx);
      const marker = await setMarker(ctx);
      const form = page.locator('form[action="/autonomy/limits"]');
      await form.locator('input[name=daily]').fill('3.5');
      const mark = ctx.reqs.length;
      await form.getByRole('button', { name: '저장' }).click();
      const post = await exactlyOnePost(ctx, mark, '/autonomy/limits');
      const body = bodyOf(post);
      eq(body.get('daily'), '3.5', '보낸 daily');
      eq(body.get('csrf'), csrf, '보낸 csrf');
      eq(body.get('back'), '/settings', '보낸 back');
      ok(body.has('weekly') && body.has('quiet') && body.has('routine_wakes'), `폼의 다른 칸이 같이 가지 않았다: ${[...body.keys()]}`);
      eq(post.contentType.split(';')[0], 'application/x-www-form-urlencoded', '일반 폼의 Content-Type');
      await expectFlash(ctx, { tone: 'ok', text: '저장했습니다. 다음 자동 실행부터 적용됩니다.' });
      ok(await form.isVisible(), '보낸 뒤 폼이 사라졌다');
      await expectNoDocumentLoad(ctx, mark, marker, '폼 보내기');
      // 안내는 "닫기"로 지워지고, 다른 화면으로 가면 사라진다.
      await page.getByRole('button', { name: '안내 닫기' }).click();
      eq(await page.locator('main#main .flash').count(), 0, '"안내 닫기"를 눌러도 안내가 남았다');
      await form.locator('input[name=daily]').fill('4');
      await form.getByRole('button', { name: '저장' }).click();
      await page.locator('main#main .flash').waitFor();
      await page.locator('nav[aria-label="주 메뉴"]').getByRole('link', { name: /^자동화/ }).click();
      await until(() => pathOf(ctx) === '/automation', '자동화로 가지 않는다');
      await settled(ctx);
      eq(await page.locator('main#main .flash').count(), 0, '다른 화면으로 갔는데 앞 화면의 안내가 남았다');
    });
    await step(ctx, '3b 키보드로 폼을 보낸 뒤 초점이 문서 맨 위로 날아가지 않는다', { m: false }, async () => {
      await open(ctx, '/settings');
      const daily = page.locator('form[action="/autonomy/limits"] input[name=daily]');
      await daily.fill('3');
      const mark = ctx.reqs.length;
      await daily.press('Enter');
      await exactlyOnePost(ctx, mark, '/autonomy/limits');
      await page.locator('main#main .flash').waitFor();
      await sleep(300);
      const focus = await page.evaluate(() => {
        const a = document.activeElement;
        return a === document.body || !a ? 'body' : `${a.tagName.toLowerCase()}${a.id ? `#${a.id}` : ''}`;
      });
      ok(focus !== 'body', `키보드로 보낸 뒤 초점이 body로 떨어졌다(보내던 칸과 단추가 화면째 다시 그려져 사라진다). 메뉴 링크를 처음부터 다시 거쳐야 한다`);
    });

    // ===== 4. 서버가 거절하는 값 =====
    await step(ctx, '4 서버의 검증 오류(role=alert)', { m: false }, async () => {
      await open(ctx, '/settings');
      const form = page.locator('form[action="/autonomy/limits"]');
      await form.locator('input[name=daily]').fill('abc');
      const mark = ctx.reqs.length;
      await form.getByRole('button', { name: '저장' }).click();
      await exactlyOnePost(ctx, mark, '/autonomy/limits');
      await expectFlash(ctx, { tone: 'bad', text: '저장하지 않았습니다: 하루 상한은 0보다 큰 달러 금액이어야 합니다' });
      ok(await form.isVisible(), '오류 뒤 폼이 사라졌다');
      const kept = await form.locator('input[name=daily]').inputValue();
      if (kept !== 'abc') ctx.notes.push(`서버가 거절한 뒤 폼이 서버 값("${kept}")으로 다시 그려져 사용자가 친 값("abc")이 사라진다`);
    });

    // ===== 5. 체크 상자 폼 =====
    for (const spec of [
      { page: '/settings', action: '/quiet/calendar', field: 'on', label: '일정 중 조용히(/quiet/calendar)', off: '일정 기반 방해 금지를 껐습니다.', on: '일정 있는 동안도 조용히 합니다' },
      { page: '/connections', action: '/app/push-preview', field: 'preview', label: '푸시 미리보기(/app/push-preview)', off: '푸시에 고정 문구만 싣습니다.', on: '푸시에 글 앞부분을 싣습니다' },
    ]) {
      await step(ctx, `5 체크 상자 폼, ${spec.label}`, { m: false }, async () => {
        await open(ctx, spec.page);
        const form = page.locator(`form[action="${spec.action}"]`);
        const box = form.locator(`input[type=checkbox][name=${spec.field}]`);
        await box.scrollIntoViewIfNeeded();
        // 끈 채로 보낸다: 필드가 없어야 한다.
        await box.setChecked(false);
        let mark = ctx.reqs.length;
        await form.getByRole('button', { name: '저장' }).click();
        let post = await exactlyOnePost(ctx, mark, spec.action);
        let body = bodyOf(post);
        ok(!body.has(spec.field), `끈 체크 상자의 ${spec.field}가 본문에 들어 있다`);
        ok(body.has('csrf'), 'csrf가 빠졌다');
        await expectFlash(ctx, { tone: 'ok', text: spec.off });
        // 켠 채로 보낸다: 상자의 value가 실려야 한다.
        await open(ctx, spec.page);
        const again = page.locator(`form[action="${spec.action}"]`);
        const box2 = again.locator(`input[type=checkbox][name=${spec.field}]`);
        await box2.scrollIntoViewIfNeeded();
        await box2.setChecked(false);
        await box2.setChecked(true);
        const value = await box2.evaluate((el) => el.value);
        mark = ctx.reqs.length;
        await again.getByRole('button', { name: '저장' }).click();
        post = await exactlyOnePost(ctx, mark, spec.action);
        body = bodyOf(post);
        eq(body.get(spec.field), value, `켠 체크 상자의 ${spec.field}`);
        await expectFlash(ctx, { tone: 'ok', text: spec.on });
      });
    }

    // ===== 6. 단추 하나짜리 폼 =====
    await step(ctx, '6 단추 하나짜리 폼(사전 체크, /run/precheck)', { m: false }, async () => {
      await open(ctx, '/');
      const csrf = await csrfOf(ctx);
      const form = page.locator('form[action="/run/precheck"]');
      const mark = ctx.reqs.length;
      await form.getByRole('button').click();
      const post = await exactlyOnePost(ctx, mark, '/run/precheck');
      const body = bodyOf(post);
      eq(body.get('csrf'), csrf, '보낸 csrf');
      eq(body.get('back'), '/', '보낸 back');
      await expectFlash(ctx, { tone: 'info', text: '사전 체크을(를) 작업 큐에 넣었습니다' });
    });
    await step(ctx, '6b 빠르게 두 번 눌러도 한 번만 보낸다', { m: false }, async () => {
      await open(ctx, '/');
      const form = page.locator('form[action="/run/precheck"]');
      const mark = ctx.reqs.length;
      await form.getByRole('button').dblclick();
      await sleep(1200);
      eq(postsTo(ctx, mark, '/run/precheck').length, 1, '두 번 누른 POST 수');
    });

    // ===== 7. 묻는 창 [m] =====
    await step(ctx, '7 묻는 창(지우기, /routines/…/remove)', {}, async () => {
      await open(ctx, '/automation');
      const csrf = await csrfOf(ctx);
      const confirmForm = page.locator('form[action^="/routines/"][action$="/remove"]').first();
      const action = await confirmForm.getAttribute('action');
      const dialog = confirmForm.locator('xpath=ancestor::dialog');
      const opener = dialog.locator('xpath=preceding-sibling::button[1]');
      const state = () =>
        dialog.evaluate((d) => ({ open: d.open, modal: d.matches(':modal'), focusIn: d.contains(document.activeElement), focus: document.activeElement?.tagName }));
      const mark = ctx.reqs.length;
      const openIt = async () => {
        await opener.scrollIntoViewIfNeeded();
        await opener.click();
        await until(async () => (await state()).open, '첫 단추를 눌러도 묻는 창이 열리지 않는다');
      };
      eq(postsTo(ctx, mark, action).length, 0, '첫 단추만으로 POST가 갔다');
      await openIt();
      const s = await state();
      ok(s.modal, '묻는 창이 :modal이 아니다');
      ok(s.focusIn, `묻는 창을 열었는데 초점이 창 밖에 있다(${s.focus})`);
      // 뒤에 놓인 링크 중 가운데가 뷰포트 안에 있고 묻는 창 상자 밖인 것을 골라(건너뛰기 링크처럼 화면 밖에 숨은 것은 뺀다),
      // 그 자리를 누르면 링크가 아니라 창(의 어두운 막)이 받는지, 링크에 초점을 줘도 못 받는지 본다.
      const behind = await page.evaluate(() => {
        const dialogEl = document.querySelector('dialog[open]');
        const box = dialogEl.getBoundingClientRect();
        const pick = [...document.querySelectorAll('a[href]')].find((a) => {
          if (a.closest('dialog')) return false;
          const r = a.getBoundingClientRect();
          if (r.width === 0 || r.height === 0) return false;
          const cx = r.left + r.width / 2;
          const cy = r.top + r.height / 2;
          const inViewport = cx >= 0 && cx < innerWidth && cy >= 0 && cy < innerHeight;
          const inDialog = cx >= box.left && cx <= box.right && cy >= box.top && cy <= box.bottom;
          return inViewport && !inDialog;
        });
        if (!pick) return null;
        const r = pick.getBoundingClientRect();
        const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        pick.focus();
        return { focused: document.activeElement === pick, hitsLink: hit === pick || pick.contains(hit), hit: hit === dialogEl ? 'dialog' : hit?.tagName.toLowerCase(), link: pick.getAttribute('href') };
      });
      ok(behind !== null, '뒤에 놓인 시험용 링크(뷰포트 안, 창 밖)를 찾지 못했다(검사 자체의 문제)');
      ok(!behind.focused, `묻는 창 뒤의 링크(${behind.link})가 초점을 받았다(뒤 화면이 inert가 아니다)`);
      ok(!behind.hitsLink, `묻는 창 뒤의 링크(${behind.link})가 포인터를 받는다`);
      eq(behind.hit, 'dialog', '뒤 화면을 누르면 받는 것(창의 어두운 막)');
      // Esc: 닫히고 요청이 없고 초점이 처음 단추로 돌아온다.
      await page.keyboard.press('Escape');
      await until(async () => !(await state()).open, 'Esc로 묻는 창이 닫히지 않는다');
      ok(await opener.evaluate((b) => document.activeElement === b), 'Esc로 닫은 뒤 초점이 처음 단추로 돌아오지 않았다');
      await sleep(500);
      eq(postsTo(ctx, mark, action).length, 0, 'Esc로 닫았는데 POST가 갔다');
      // 닫기 단추.
      await openIt();
      await dialog.getByRole('button', { name: '닫기' }).click();
      await until(async () => !(await state()).open, '"닫기" 단추로 묻는 창이 닫히지 않는다');
      await sleep(500);
      eq(postsTo(ctx, mark, action).length, 0, '"닫기"를 눌렀는데 POST가 갔다');
      // 확인.
      await openIt();
      await dialog.getByRole('button', { name: '지우기' }).click();
      const post = await exactlyOnePost(ctx, mark, action, `${action}(확인)`);
      const body = bodyOf(post);
      eq(body.get('csrf'), csrf, '보낸 csrf');
      eq(body.get('back'), '/automation', '보낸 back');
      // 시험 서버의 저장소는 비어 있어 이 루틴이 없다: 처리기는 오류 안내로 답한다(왕복은 맞다).
      await expectFlash(ctx, { tone: 'bad', text: '없는 루틴입니다.' });
      ok(!(await page.locator('dialog[open]').count()), '확인한 뒤에도 열린 창이 남았다');
    });

    // ===== 8. 파일 올리기 =====
    await step(ctx, '8 파일 올리기(multipart, /expressions/add)', { m: false }, async () => {
      await open(ctx, '/expressions');
      const csrf = await csrfOf(ctx);
      const form = page.locator('form[action="/expressions/add"]');
      const png = makePng(32, 32);
      await form.locator('input[type=file][name=file]').setInputFiles({ name: 'flow-test.png', mimeType: 'image/png', buffer: png });
      await form.locator('input[name=name]').fill('flow_smile');
      await form.locator('input[name=label]').fill('흐름 시험');
      const mark = ctx.reqs.length;
      const formsBefore = ctx.forms.length;
      await form.getByRole('button', { name: '올리기' }).click();
      const post = await exactlyOnePost(ctx, mark, '/expressions/add');
      ok(/^multipart\/form-data; boundary=/.test(post.contentType), `Content-Type이 multipart가 아니다: ${post.contentType}`);
      // 실제로 나간 본문(받을 수 있으면)과 앱이 만든 FormData 모두에서 파일 부분을 본다.
      const wire = post.request.postDataBuffer();
      if (wire) {
        const text = wire.toString('latin1');
        ok(/name="file"; filename="flow-test\.png"/.test(text), '실제 본문에 file 부분(파일 이름)이 없다');
        ok(wire.includes(png.subarray(0, 8)), '실제 본문에 PNG 바이트가 없다');
        ok(/name="csrf"\r\n\r\n/.test(text) && text.includes(csrf), '실제 본문에 csrf 부분이 없다');
      } else {
        ctx.notes.push('Playwright가 multipart 본문을 주지 않아 FormData 기록으로만 확인했다');
      }
      const sent = ctx.forms.slice(formsBefore).find((f) => f.url.endsWith('/expressions/add'));
      ok(sent, '앱이 보낸 FormData 기록이 없다');
      const entries = Object.fromEntries(sent.entries);
      eq(entries.csrf, csrf, 'FormData의 csrf');
      eq(entries.back, '/expressions', 'FormData의 back');
      eq(entries.name, 'flow_smile', 'FormData의 name');
      eq(entries.kind, 'emoji', 'FormData의 kind');
      eq(JSON.stringify(entries.file), JSON.stringify({ file: 'flow-test.png', type: 'image/png', size: png.length }), 'FormData의 file 부분');
      // 시험 서버에는 접속 API(gateway)가 없어 처리기가 "이모지·스티커를 다루지 않는다"고 거절한다(받는 쪽까지 닿았다는 뜻).
      await expectFlash(ctx, { tone: 'bad', text: '이 데몬은 이모지·스티커를 다루지 않습니다.' });
    });


    // ===== 8b~8d. 초기 설정(결정 0064) =====
    // 화면 데이터는 묶음 머리(`x-fixture-set`)로 고르고, 쓰기는 실제 처리기가 받는다(저장소는 비어 있다).
    const withSet = async (set, run) => {
      await ctx.context.setExtraHTTPHeaders({ 'x-fixture-set': set });
      try {
        await run();
      } finally {
        await ctx.context.setExtraHTTPHeaders({});
      }
    };
    await step(ctx, '8b 초기 설정: 처리기가 연결 화면으로 돌려보내도 이 화면에 남는다(stay)', { m: false }, async () => {
      await withSet('fresh', async () => {
        await open(ctx, '/start');
        eq(await h1Of(ctx), '초기 설정', '제목');
        const marker = await setMarker(ctx);
        // 이름: 저장소만 쓰는 처리기. 성공해도 이 화면에 있다.
        const nameForm = page.locator('form[action="/profile/name"]');
        await nameForm.locator('input[name=name]').fill('달빛');
        let mark = ctx.reqs.length;
        await nameForm.getByRole('button', { name: '저장' }).click();
        const post = await exactlyOnePost(ctx, mark, '/profile/name');
        eq(bodyOf(post).get('name'), '달빛', '보낸 name');
        eq(bodyOf(post).get('back'), '/start', '보낸 back');
        await expectFlash(ctx, { tone: 'ok', text: '이름을 “달빛”(으)로 정했습니다' });
        eq(pathOf(ctx), '/start', '이름을 저장한 뒤 주소');
        await expectNoDocumentLoad(ctx, mark, marker, '이름 저장');
        // 연결 화면의 처리기(/images/key): 늘 /connections로 돌려보내지만 이 화면에 남고, 거절돼도 친 값이 남는다.
        const keyForm = page.locator('form[action="/images/key"]');
        await keyForm.locator('input[name=key]').fill('short');
        mark = ctx.reqs.length;
        await keyForm.getByRole('button', { name: '저장' }).click();
        await exactlyOnePost(ctx, mark, '/images/key');
        await expectFlash(ctx, { tone: 'bad', text: '키를 저장하지 않았습니다' });
        eq(pathOf(ctx), '/start', '키를 거절당한 뒤 주소');
        eq(await keyForm.locator('input[name=key]').inputValue(), 'short', '거절당한 뒤 친 값이 지워졌다');
        const fetched = ctx.reqs.slice(mark).map((r) => r.path);
        ok(!fetched.includes('/connections'), `연결 화면을 받았다: ${fetched}`);
        await expectNoDocumentLoad(ctx, mark, marker, '키 저장');
      });
    });
    await step(ctx, '8c 초기 설정: 프로필 그림 올리기(multipart)와 지우기(묻는 창)', { m: false }, async () => {
      await withSet('fresh', async () => {
        await open(ctx, '/start');
        const csrf = await csrfOf(ctx);
        const form = page.locator('form[action="/profile/avatar"]');
        const png = makePng(256, 256);
        await form.locator('input[type=file][name=file]').setInputFiles({ name: 'avatar.png', mimeType: 'image/png', buffer: png });
        const mark = ctx.reqs.length;
        const formsBefore = ctx.forms.length;
        await form.getByRole('button', { name: '올리기' }).click();
        const post = await exactlyOnePost(ctx, mark, '/profile/avatar');
        ok(/^multipart\/form-data; boundary=/.test(post.contentType), `Content-Type이 multipart가 아니다: ${post.contentType}`);
        const sent = ctx.forms.slice(formsBefore).find((f) => f.url.endsWith('/profile/avatar'));
        ok(sent, '앱이 보낸 FormData 기록이 없다');
        const entries = Object.fromEntries(sent.entries);
        eq(entries.csrf, csrf, 'FormData의 csrf');
        eq(entries.back, '/start', 'FormData의 back');
        eq(JSON.stringify(entries.file), JSON.stringify({ file: 'avatar.png', type: 'image/png', size: png.length }), 'FormData의 file 부분');
        // 시험 서버는 프로필(앱·Discord 없이)을 붙여 둔다: 실제로 정해지고, Discord는 연결되어 있지 않다고 알린다.
        const flash = await expectFlash(ctx, { tone: 'ok', text: '프로필 그림을 바꿨습니다(256×256' });
        ok(flash.text.includes('Discord는 연결되어 있지 않아'), `Discord 쪽 안내가 없다: ${flash.text}`);
        eq(pathOf(ctx), '/start', '올린 뒤 주소');
        // 규칙을 어기면(정사각형이 아님) 어긴 것을 알리고 이 화면에 남는다.
        await form.locator('input[type=file][name=file]').setInputFiles({ name: 'wide.png', mimeType: 'image/png', buffer: makePng(300, 200) });
        const again = ctx.reqs.length;
        await form.getByRole('button', { name: '올리기' }).click();
        await exactlyOnePost(ctx, again, '/profile/avatar');
        await expectFlash(ctx, { tone: 'bad', text: '정사각형이 아니다(300×200)' });
        eq(pathOf(ctx), '/start', '거절당한 뒤 주소');
      });
      // 그림이 있는 묶음에서 지운다(저장소에는 방금 올린 그림이 있다).
      await withSet('full', async () => {
        await open(ctx, '/start');
        const csrf = await csrfOf(ctx);
        const confirmForm = page.locator('form[action="/profile/avatar/remove"]');
        const dialog = confirmForm.locator('xpath=ancestor::dialog');
        const mark = ctx.reqs.length;
        await page.getByRole('button', { name: '그림 지우기' }).first().click();
        await until(() => dialog.evaluate((d) => d.open), '"그림 지우기"를 눌러도 묻는 창이 열리지 않는다');
        eq(postsTo(ctx, mark, '/profile/avatar/remove').length, 0, '첫 단추만으로 POST가 갔다');
        await dialog.getByRole('button', { name: '지우기' }).click();
        const post = await exactlyOnePost(ctx, mark, '/profile/avatar/remove');
        eq(bodyOf(post).get('csrf'), csrf, '보낸 csrf');
        eq(bodyOf(post).get('back'), '/start', '보낸 back');
        await expectFlash(ctx, { tone: 'ok', text: '프로필 그림을 지웠습니다' });
        eq(pathOf(ctx), '/start', '지운 뒤 주소');
      });
    });
    await step(ctx, '8d 초기 설정: 로그인 뒤 첫 화면(안내를 켜면 초기 설정, 끄면 홈)', { m: false }, async () => {
      const landing = async () => {
        const r = await ctx.context.request.post(`${ctx.base}/login`, {
          form: { password: PASSWORD },
          headers: { Accept: 'application/json' },
          maxRedirects: 0,
        });
        ok(r.ok(), `API 로그인 실패(HTTP ${r.status()})`);
        return (await r.json()).redirect;
      };
      eq(await landing(), '/', '안내를 끈 채 시작한 시험 서버의 첫 화면');
      // 끝나지 않았는데 안내를 끈 묶음에서 "다시 켜기": 실제로 저장소가 바뀐다.
      await withSet('troubled', async () => {
        await open(ctx, '/start');
        const mark = ctx.reqs.length;
        await page.getByRole('button', { name: '홈 안내 다시 켜기' }).click();
        await exactlyOnePost(ctx, mark, '/start/reopen');
        await expectFlash(ctx, { tone: 'ok', text: '다시 보입니다' });
        eq(pathOf(ctx), '/start', '다시 켠 뒤 주소(이 화면에 머문다)');
      });
      eq(await landing(), '/start', '안내를 켜면 새 설치의 첫 화면');
      // 안내를 끄는 단추: 홈으로 간다.
      await withSet('fresh', async () => {
        await open(ctx, '/start');
        const mark = ctx.reqs.length;
        await page.getByRole('button', { name: '나중에 하기' }).click();
        await exactlyOnePost(ctx, mark, '/start/dismiss');
        await until(() => pathOf(ctx) === '/', '안내를 끈 뒤 홈으로 가지 않는다');
        await expectFlash(ctx, { tone: 'ok', text: '안내를 껐습니다' });
      });
      eq(await landing(), '/', '안내를 끄면 홈');
    });

    // ===== 9. GET 찾기 폼 =====
    await step(ctx, '9 찾기 폼(GET, /memory?q=)', { m: false }, async () => {
      await open(ctx, '/memory');
      const marker = await setMarker(ctx);
      const search = page.locator('form[role=search]');
      const mark = ctx.reqs.length;
      await search.locator('input[name=q]').fill('qz');
      await search.locator('input[name=q]').press('Enter');
      await until(() => pathOf(ctx) === '/memory?q=qz', `찾기 뒤 주소가 /memory?q=qz가 아니다(${pathOf(ctx)})`);
      await page.getByText('개 찾음').waitFor();
      ok(await page.getByRole('link', { name: '검색 지우기' }).isVisible(), '"검색 지우기" 링크가 없다');
      eq(await page.locator('form[role=search] input[name=q]').inputValue(), 'qz', '찾는 말 칸');
      const posts = ctx.reqs.slice(mark).filter((r) => r.method !== 'GET');
      eq(posts.length, 0, `찾기가 GET 밖의 요청을 보냈다 ${JSON.stringify(posts.map((p) => `${p.method} ${p.path}`))}`);
      await expectNoDocumentLoad(ctx, mark, marker, '찾기');
    });

    // ===== 10. 저장하지 않은 변경 지킴 =====
    await step(ctx, '10 저장하지 않은 변경(메뉴 이동)', { m: false }, async () => {
      await open(ctx, '/settings');
      const input = page.locator('form[action="/autonomy/limits"] input[name=daily]');
      await input.fill('9');
      const mark = ctx.reqs.length;
      const stay = expectDialog(ctx, 'dismiss');
      await page.locator('nav[aria-label="주 메뉴"]').getByRole('link', { name: /^자동화/ }).click();
      await until(() => stay.seen, '변경이 있는데 떠날 때 묻는 창(confirm)이 뜨지 않는다');
      eq(stay.seen.type, 'confirm', '묻는 창의 종류');
      ok(stay.seen.message.includes('저장하지 않은 변경'), `묻는 창의 글: ${stay.seen.message}`);
      await sleep(400);
      eq(pathOf(ctx), '/settings', '취소했는데 주소가 바뀌었다');
      eq(await input.inputValue(), '9', '취소했는데 친 값이 사라졌다');
      eq(ctx.reqs.slice(mark).filter((r) => r.path === '/automation').length, 0, '취소했는데 /automation을 물었다');
      const leave = expectDialog(ctx, 'accept');
      await page.locator('nav[aria-label="주 메뉴"]').getByRole('link', { name: /^자동화/ }).click();
      await until(() => pathOf(ctx) === '/automation', '확인했는데 /automation으로 가지 않는다');
      ok(leave.seen, '두 번째에는 묻는 창이 뜨지 않았다');
      await settled(ctx);
      eq(await h1Of(ctx), '자동화', '확인한 뒤 제목');
    });
    await step(ctx, '10b 저장하지 않은 변경(뒤로 가기)', { m: false }, async () => {
      await open(ctx, '/');
      await page.locator('nav[aria-label="주 메뉴"]').getByRole('link', { name: /^설정/ }).click();
      await until(() => pathOf(ctx) === '/settings', '설정으로 가지 않는다');
      await settled(ctx);
      const input = page.locator('form[action="/autonomy/limits"] input[name=daily]');
      await input.fill('8');
      const stay = expectDialog(ctx, 'dismiss');
      await page.goBack();
      await until(() => stay.seen, '변경이 있는데 뒤로 갈 때 묻는 창이 뜨지 않는다');
      await sleep(400);
      eq(pathOf(ctx), '/settings', '취소했는데 주소가 /settings가 아니다');
      eq(await input.inputValue(), '8', '취소했는데 친 값이 사라졌다');
      const leave = expectDialog(ctx, 'accept');
      await page.goBack();
      // 뒤로 가기는 주소가 먼저 바뀌고 popstate에서 묻는다: 창이 뜬 것부터 확인한다.
      await until(() => leave.seen, '두 번째 뒤로 가기에는 묻는 창이 뜨지 않았다');
      await until(async () => pathOf(ctx) === '/' && (await h1Of(ctx)) === '홈', `확인했는데 이전 화면으로 가지 않는다(${pathOf(ctx)})`);
    });

    // ===== 11. 원격 화면 누르기 [m] =====
    await step(ctx, '11 원격 화면 누르기(좌표)', {}, async () => {
      await open(ctx, '/browser/handover?tab=qz-tab-one');
      const csrf = await csrfOf(ctx);
      const image = page.locator('form[action="/browser/handover/act"] input[type=image][name=at]');
      await image.waitFor();
      // 그림이 다 그려져 폭에 맞게 줄었을 때까지(높이 = 폭 x 800/1365).
      const rectOf = () => image.evaluate((el) => {
        const r = el.getBoundingClientRect();
        return { left: r.left, top: r.top, width: r.width, height: r.height };
      });
      await until(async () => {
        const r = await rectOf();
        return r.width > 0 && Math.abs(r.height - (r.width * 800) / 1365) < 2;
      }, '원격 화면 그림이 가로세로 비율대로 그려지지 않는다');
      // 그림의 원래 크기(시험 서버가 주는 1365x800).
      const natural = await page.evaluate((src) => new Promise((done) => {
        const probe = new Image();
        probe.onload = () => done({ w: probe.naturalWidth, h: probe.naturalHeight });
        probe.onerror = () => done(null);
        probe.src = src;
      }), await image.evaluate((el) => el.src));
      eq(JSON.stringify(natural), JSON.stringify({ w: 1365, h: 800 }), '시험 서버가 주는 그림의 원래 크기');
      const clickAt = async (fx, fy, name) => {
        await image.scrollIntoViewIfNeeded();
        await until(async () => (await rectOf()).width > 0, '그림 자리를 잴 수 없다');
        const r = await rectOf();
        // 정수 좌표를 눌러, 반올림이 기대값을 흔들지 않게 한다.
        const x = Math.round(r.left + r.width * fx);
        const y = Math.round(r.top + r.height * fy);
        const hit = await page.evaluate(([px, py]) => {
          const el = document.elementFromPoint(px, py);
          return el ? `${el.tagName.toLowerCase()}${el.getAttribute('type') ? `[type=${el.getAttribute('type')}]` : ''}` : null;
        }, [x, y]);
        eq(hit, 'input[type=image]', `${name}: 누를 자리에 있는 요소(다른 것이 가리고 있다)`);
        const scale = natural.w / r.width;
        const expectX = ((x - r.left) * natural.w) / r.width;
        const expectY = ((y - r.top) * natural.h) / r.height;
        const mark = ctx.reqs.length;
        if (ctx.vp.mobile) await page.touchscreen.tap(x, y);
        else await page.mouse.click(x, y);
        const post = await exactlyOnePost(ctx, mark, '/browser/handover/act', `${name}: /browser/handover/act`);
        const body = bodyOf(post);
        eq(body.get('tab'), 'qz-tab-one', `${name}: tab`);
        eq(body.get('kind'), 'click', `${name}: kind`);
        eq(body.get('csrf'), csrf, `${name}: csrf`);
        const sentX = Number(body.get('at.x'));
        const sentY = Number(body.get('at.y'));
        ok(Number.isFinite(sentX) && Number.isFinite(sentY), `${name}: at.x/at.y가 숫자가 아니다(${body.get('at.x')}, ${body.get('at.y')})`);
        near(sentX, expectX, 2, `${name}: at.x(원래 그림 좌표, 화면 ${x - r.left}px, 배율 ${scale.toFixed(2)})`);
        near(sentY, expectY, 2, `${name}: at.y(원래 그림 좌표, 화면 ${y - r.top}px)`);
        // 눈으로 가늠하는 값(전체의 fx·fy)과도 맞는지: 정수 반올림이 배율만큼 흔들 수 있어 그만큼 더 허용한다.
        near(sentX, natural.w * fx, 2 + Math.ceil(scale / 2), `${name}: at.x가 그림 가로의 ${fx * 100}% 근처`);
        near(sentY, natural.h * fy, 2 + Math.ceil(scale / 2), `${name}: at.y가 그림 세로의 ${fy * 100}% 근처`);
        return { sentX, sentY, scale, width: r.width };
      };
      const first = await clickAt(0.25, 0.5, '25%·50%');
      ctx.notes.push(`그림 ${first.width.toFixed(0)}px로 표시(배율 ${first.scale.toFixed(2)}), 보낸 좌표 ${first.sentX},${first.sentY}`);
      // 눌러서 화면이 다시 그려진 뒤(처리기는 안내 없이 되돌아온다) 다른 자리도 본다.
      await settled(ctx);
      await until(async () => {
        const r = await rectOf();
        return r.width > 0 && Math.abs(r.height - (r.width * 800) / 1365) < 2;
      }, '두 번째 누르기 전에 그림이 다시 그려지지 않는다');
      await clickAt(0.8, 0.2, '80%·20%');
    });

    await step(ctx, '11b 키보드로 원격 화면을 눌러도 (0,0)을 누르지 않는다', { m: false }, async () => {
      await open(ctx, '/browser/handover?tab=qz-tab-one');
      const image = page.locator('form[action="/browser/handover/act"] input[type=image][name=at]');
      await image.waitFor();
      await sleep(500);
      await image.focus();
      const mark = ctx.reqs.length;
      await page.keyboard.press('Enter');
      await sleep(1200);
      const posts = postsTo(ctx, mark, '/browser/handover/act').map(bodyOf);
      const corner = posts.filter((b) => b.get('kind') === 'click' && b.get('at.x') === '0' && b.get('at.y') === '0');
      eq(corner.length, 0, '키보드로 원격 화면 단추를 활성화했더니 원격 화면의 왼쪽 위 모서리(0,0)를 눌렀다');
    });

    // ===== 12. 자동 새로고침 =====
    await step(ctx, '12 자동 새로고침(3초마다)', { m: false }, async () => {
      // 시험 서버의 화면 데이터는 늘 같은 시각(snapshot.now)을 준다. 진짜 서버는 물을 때마다 새 시각이므로(그림 주소의 v= 값이
      // 거기서 온다), 그 점만 답에서 바꿔 준다. 그렇지 않으면 그림 주소가 안 바뀌어 새 그림을 받을 일이 없다.
      let bump = 0;
      const handler = async (route) => {
        const request = route.request();
        if (!(request.headers().accept ?? '').includes('application/json')) return route.continue();
        const response = await route.fetch();
        const body = await response.json();
        bump++;
        body.snapshot.now = new Date(Date.parse(body.snapshot.now) + bump * 1000).toISOString();
        return route.fulfill({ response, json: body });
      };
      const matcher = (url) => url.pathname === '/browser/handover';
      await page.route(matcher, handler);
      try {
        await open(ctx, '/browser/handover?tab=qz-tab-one&auto=1');
        await page.locator('form[action="/browser/handover/act"] input[type=image]').waitFor();
        await sleep(300);
        const mark = ctx.reqs.length;
        const jsonGets = () => ctx.reqs.slice(mark).filter((r) => r.method === 'GET' && r.path === '/browser/handover' && r.accept.includes('application/json'));
        const frames = () => ctx.reqs.slice(mark).filter((r) => r.path.startsWith('/browser/handover/frame/'));
        const src0 = await page.locator('form[action="/browser/handover/act"] input[type=image]').getAttribute('src');
        await sleep(7000);
        ok(jsonGets().length >= 2, `7초 동안 화면 데이터를 다시 물은 횟수가 ${jsonGets().length}번이다(3초마다면 2번 이상)`);
        const src1 = await page.locator('form[action="/browser/handover/act"] input[type=image]').getAttribute('src');
        ok(src0 !== src1, `그림 주소가 바뀌지 않았다(${src0})`);
        ok(frames().length >= 1, '새 그림을 다시 받지 않았다');
        // 떠나면 멈춘다.
        await page.locator('nav[aria-label="주 메뉴"]').getByRole('link', { name: /^홈/ }).click();
        await until(() => pathOf(ctx) === '/', '홈으로 가지 않는다');
        await settled(ctx);
        await sleep(600);
        const leftAt = ctx.reqs.length;
        await sleep(4000);
        const later = ctx.reqs.slice(leftAt).filter((r) => r.path === '/browser/handover' || r.path.startsWith('/browser/handover/frame/'));
        eq(later.length, 0, `떠난 뒤에도 원격 화면을 계속 물었다 ${JSON.stringify(later.map((r) => r.path + r.search))}`);
      } finally {
        await page.unroute(matcher, handler);
      }
    });

    // ===== 12b. 두 단 배치: 접힌 것을 펼쳐도 구역이 옆 단으로 옮겨 가지 않는다 =====
    // 높이 맞추기를 브라우저에만 맡기면 내용이 늘 때마다 단을 다시 나눠, 쓰던 구역이 옆으로 튄다(Columns가 못 박는다).
    await step(ctx, '12b 접힌 것을 펼쳐도 구역이 옆 단으로 옮겨 가지 않는다', { m: false }, async () => {
      for (const url of ['/connections', '/settings', '/settings/work', '/work', '/browser', '/character', '/expressions']) {
        await open(ctx, url);
        const moved = await page.evaluate(async () => {
          const frame = () => new Promise((done) => requestAnimationFrame(() => done()));
          const lefts = () =>
            Object.fromEntries(
              [...document.querySelectorAll('.columns > section')].map((s) => [s.id, Math.round(s.getBoundingClientRect().left)]),
            );
          const before = lefts();
          const out = [];
          for (const details of document.querySelectorAll('.columns details')) {
            details.open = true;
            await frame();
            const now = lefts();
            const changed = Object.keys(before).filter((id) => before[id] !== now[id]);
            if (changed.length > 0) out.push(`"${(details.querySelector('summary')?.textContent ?? '').trim()}"을 펼치자 ${changed.join(', ')} 구역이 옮겨 갔다`);
            details.open = false;
            await frame();
          }
          return out;
        });
        ok(moved.length === 0, `${url}: ${moved.join(' / ')}`);
      }
    });

    // ===== 13. 화면 판(테마) 바꾸기 =====
    await step(ctx, '13 화면 판 바꾸기(테마)', { m: false }, async () => {
      await open(ctx, '/');
      const button = () => page.getByRole('button', { name: /^화면:/ });
      const read = () =>
        page.evaluate(() => ({
          theme: document.documentElement.dataset.theme ?? null,
          background: getComputedStyle(document.body).backgroundColor,
          stored: (() => {
            try {
              return localStorage.getItem('lucia-admin-theme');
            } catch {
              return 'x';
            }
          })(),
        }));
      const label = async () => (await button().innerText()).trim();
      eq(await label(), '화면: 기기 설정', '처음 단추 글');
      const system = await read();
      eq(system.theme, null, '처음 data-theme');
      await button().click();
      await until(async () => (await read()).theme === 'light', '첫 클릭 뒤 data-theme이 light가 아니다');
      eq(await label(), '화면: 밝게', '둘째 단추 글');
      const light = await read();
      await button().click();
      await until(async () => (await read()).theme === 'dark', '둘째 클릭 뒤 data-theme이 dark가 아니다');
      eq(await label(), '화면: 어둡게', '셋째 단추 글');
      const dark = await read();
      ok(dark.background !== light.background, `어두운 판과 밝은 판의 배경색이 같다(${dark.background})`);
      eq(light.background, system.background, '기기 설정(밝은 기기)과 밝은 판의 배경색');
      eq(dark.stored, 'dark', '고른 판이 저장되지 않았다');
      // 새로 고쳐도 유지.
      await page.reload();
      await settled(ctx);
      const after = await read();
      eq(after.theme, 'dark', '새로 고친 뒤 data-theme');
      eq(after.background, dark.background, '새로 고친 뒤 배경색');
      eq(await label(), '화면: 어둡게', '새로 고친 뒤 단추 글');
      await button().click();
      await until(async () => (await read()).theme === null, '셋째 클릭 뒤 data-theme이 지워지지 않는다');
      eq(await label(), '화면: 기기 설정', '한 바퀴 돈 단추 글');
      eq((await read()).stored, null, '기기 설정으로 돌아갔는데 저장값이 남았다');
    });

    // ===== 15a. 응답 머리 =====
    await step(ctx, '15a 보안 머리(틀과 JSON 화면)', { m: false }, async () => {
      const check = (what, response) => {
        const h = response.headers();
        const csp = h['content-security-policy'] ?? '';
        ok(csp.includes("script-src 'self'"), `${what}: Content-Security-Policy에 script-src 'self'가 없다(${csp.slice(0, 80)})`);
        eq(h['x-frame-options'], 'DENY', `${what}: X-Frame-Options`);
        eq(h['x-content-type-options'], 'nosniff', `${what}: X-Content-Type-Options`);
      };
      const shell = await ctx.context.request.get(`${ctx.base}/`, { headers: { Accept: 'text/html' } });
      ok((shell.headers()['content-type'] ?? '').includes('text/html'), '틀의 Content-Type이 html이 아니다');
      check('틀(/)', shell);
      const html = await shell.text();
      ok(!/<script(?![^>]*\bsrc=)[^>]*>/.test(html), '틀에 인라인 스크립트가 있다');
      const json = await ctx.context.request.get(`${ctx.base}/automation`, { headers: { Accept: 'application/json' } });
      ok((json.headers()['content-type'] ?? '').includes('application/json'), 'JSON 화면의 Content-Type');
      check('JSON 화면(/automation)', json);
    });

    // ===== 16. 오류 경계 =====
    await step(ctx, '16 오류 경계(화면 하나가 깨져도 틀은 남는다)', { m: false, expect: { console: /./ } }, async (allowance) => {
      // /settings의 화면 데이터에서 오늘 사용량(snapshot.today)을 비운다: 화면이 그 값을 읽다 던진다. 틀이 쓰는 값(overall·notices·pending·page)은 그대로다.
      const handler = async (route) => {
        const request = route.request();
        if (!(request.headers().accept ?? '').includes('application/json')) return route.continue();
        const response = await route.fetch();
        const body = await response.json();
        body.snapshot.today = null;
        return route.fulfill({ response, json: body });
      };
      const matcher = (url) => url.pathname === '/settings';
      await page.route(matcher, handler);
      try {
        await open(ctx, '/');
        const marker = await setMarker(ctx);
        const mark = ctx.reqs.length;
        await page.locator('nav[aria-label="주 메뉴"]').getByRole('link', { name: /^설정/ }).click();
        await page.getByText('이 화면을 그리지 못했습니다.').waitFor();
        ok(await page.getByRole('alert').filter({ hasText: '이 화면을 그리지 못했습니다.' }).isVisible(), '오류 안내가 role=alert가 아니다');
        ok(await page.locator('nav[aria-label="주 메뉴"]').isVisible(), '화면이 깨졌는데 메뉴가 사라졌다');
        ok(await page.getByRole('button', { name: '로그아웃' }).isVisible(), '화면이 깨졌는데 로그아웃 단추가 사라졌다');
        ok(allowance.seen >= 1, '오류 경계가 콘솔에 오류를 남기지 않았다(기대한 console.error가 없다)');
        // 다른 화면으로 갈 수 있다.
        await page.locator('nav[aria-label="주 메뉴"]').getByRole('link', { name: /^자동화/ }).click();
        await until(async () => pathOf(ctx) === '/automation' && (await h1Of(ctx)) === '자동화', '깨진 화면에서 다른 화면(자동화)으로 가지지 않는다');
        eq(await page.getByText('이 화면을 그리지 못했습니다.').count(), 0, '다른 화면으로 갔는데 오류 안내가 남았다');
        await expectNoDocumentLoad(ctx, mark, marker, '오류 경계 시험의 이동');
        // 다시 깨진 화면으로 가서, 서버가 고쳐진 뒤 "다시 불러오기"로 복구된다.
        await page.locator('nav[aria-label="주 메뉴"]').getByRole('link', { name: /^설정/ }).click();
        await page.getByText('이 화면을 그리지 못했습니다.').waitFor();
        await page.unroute(matcher, handler);
        await page.getByRole('button', { name: '다시 불러오기' }).click();
        await until(async () => pathOf(ctx) === '/settings' && (await page.getByText('이 화면을 그리지 못했습니다.').count()) === 0 && (await h1Of(ctx)) === '설정', '"다시 불러오기"가 화면을 되살리지 못한다');
      } finally {
        await page.unroute(matcher, handler).catch(() => {});
      }
    });

    // ===== 14. 로그아웃 =====
    await step(ctx, '14 로그아웃', { m: false }, async () => {
      await open(ctx, '/');
      const marker = await setMarker(ctx);
      const mark = ctx.reqs.length;
      await page.getByRole('button', { name: '로그아웃' }).click();
      const post = await exactlyOnePost(ctx, mark, '/logout');
      ok(bodyOf(post).has('csrf'), '로그아웃 POST에 csrf가 없다');
      await page.waitForURL(`${ctx.base}/login`);
      await page.getByLabel('관리 비밀번호').waitFor();
      await expectNoDocumentLoad(ctx, mark, marker, '로그아웃');
      ok(!(await ctx.context.cookies()).some((c) => c.name === 'lucia_admin'), '로그아웃했는데 세션 쿠키가 남았다');
      const gone = await ctx.context.request.get(`${ctx.base}/`, { headers: { Accept: 'application/json' } });
      eq(gone.status(), 401, '로그아웃 뒤 JSON GET /의 상태');
      eq((await gone.json()).redirect, '/login', '로그아웃 뒤 JSON 답의 redirect');
      const csrfless = await ctx.context.request.post(`${ctx.base}/run/precheck`, { form: { csrf: 'x' }, headers: { Accept: 'application/json' }, maxRedirects: 0 });
      ok(csrfless.status() === 401 || csrfless.status() === 200, `로그아웃 뒤 쓰기 요청이 예상 밖 상태 ${csrfless.status()}`);
      eq((await csrfless.json()).redirect, '/login', '로그아웃 뒤 쓰기 요청의 redirect');
    });

    // ===== 15b. 전체 진단 =====
    await step(ctx, '15b 콘솔·CSP·다른 출처(맥락 전체)', {}, async () => {
      const bad = ctx.diag.events;
      ok(bad.length === 0, `예상 밖 진단 ${bad.length}건: ${bad.slice(0, 6).map((e) => `[${e.step}] ${e.kind}: ${e.text}`).join(' | ')}`);
    });
  } finally {
    await ctx.context.close();
  }
}

try {
  for (const key of opts.viewports) await runViewport(VIEWPORTS[key]);
} finally {
  await browser.close();
}

const failed = results.filter((r) => !r.ok);
console.log(`\n흐름 ${results.length}건 중 실패 ${failed.length}건, ${((Date.now() - startedAt) / 1000).toFixed(1)}초`);
for (const r of failed) console.log(`  실패: ${r.label} — ${r.problem}`);
process.exit(failed.length > 0 ? 1 : 0);
