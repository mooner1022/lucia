// 관리 화면 배치 검사: 실제 브라우저로 화면마다 "격자가 깨짐·글이 겹침"을 잡고, 사람이 볼 전체 화면 스크린샷을 남긴다(결정 0061).
//
// 이 스크립트는 서버를 띄우지 않는다. 먼저 화면(SPA)을 빌드하고 시험 데이터 서버를 띄워 둔다(저장소 루트에서):
//
//   (cd web/admin && npm run build)          # dist를 데몬 바이너리에 넣는다(build.rs가 dist를 지켜본다)
//   LUCIA_UI_FIXTURE_ADDR=127.0.0.1:47890 cargo test -p luciad serve_ui_fixtures -- --ignored --nocapture
//
// 그 서버를 둔 채 다른 셸에서(web/admin에서, Node는 ~/.local/opt/node/bin):
//
//   node e2e/check.mjs [--base http://127.0.0.1:47890] [--sets stress,full,alt] [--only <이름·주소 일부>]
//                      [--viewports 360x780,768x1024,1280x900] [--theme light|dark|both] [--shots] [--standalone]
//                      [--motion]   움직이는 장식(바탕의 꽃잎)을 켠 채로 잰다. 기본은 움직임 줄이기(꽃잎 없음)로 잰다
//                      [--urls /,/work,…]   시험 데이터 목록 대신 이 주소들만 본다(실제 데이터를 띄운 서버용)
//                      [--out e2e/out]
//
// 기본값: 묶음 stress,full,alt / 뷰포트 셋 모두 / 밝은 판 / 스크린샷 없음. 끝나면 서버를 끝낸다(pkill -f '[s]erve_ui_fixtures': 대괄호 꼴이어야 자기 셸을 죽이지 않는다).
// 종료 코드: 0 = 실패 없음(경고는 실패가 아니다), 1 = 발견이 있다, 2 = 실행하지 못했다(서버 없음, 로그인 실패, 인자 오류).
// 전체 발견은 <out>/report.json, 스크린샷은 <out>/shots/<묶음>/<판>-<너비>/<화면>.png(<details>를 펼치기 전, 화면이 처음 열린 모습).
// 로그인 없이 보는 화면(--standalone)은 뷰포트마다 한 번씩만, 첫 번째 판으로 본다.
// 환경 변수: LUCIA_E2E_PASSWORD(기본 fixture-password-123), LUCIA_E2E_LIBS, LUCIA_E2E_FONTS(launch.mjs 머리글 참고).

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { request } from 'playwright';
import { installProbes, isFailure, runAllChecks, watchConsole } from './checks.mjs';
import { launchBrowser } from './launch.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const SCREEN_LIMIT = 16000; // Chromium이 한 장으로 그릴 수 있는 높이의 안전한 상한(px)

// ---------- 인자 ----------
function usage(message) {
  if (message) console.error(`인자 오류: ${message}\n`);
  console.error(
    '사용법: node e2e/check.mjs [--base URL] [--sets a,b] [--only 글] [--viewports 360x780,…] [--theme light|dark|both] [--shots] [--standalone] [--motion] [--out 디렉터리]',
  );
  process.exit(2);
}

function parseArgs(argv) {
  const opts = {
    base: 'http://127.0.0.1:47890',
    sets: ['stress', 'full', 'alt'],
    only: '',
    urls: null,
    viewports: ['360x780', '768x1024', '1280x900'],
    themes: ['light'],
    shots: false,
    standalone: false,
    motion: false,
    out: join(here, 'out'),
  };
  const flags = new Set(['--shots', '--standalone', '--motion']);
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--help' || arg === '-h') usage();
    if (!arg.startsWith('--')) usage(`알 수 없는 인자 ${arg}`);
    if (flags.has(arg)) {
      opts[arg.slice(2)] = true;
      continue;
    }
    const value = argv[++i];
    if (value === undefined) usage(`${arg}에 값이 없습니다`);
    switch (arg) {
      case '--base': opts.base = value.replace(/\/$/, ''); break;
      case '--sets': opts.sets = value.split(',').filter(Boolean); break;
      case '--only': opts.only = value; break;
      case '--urls': opts.urls = value.split(',').filter(Boolean); break;
      case '--viewports': opts.viewports = value.split(',').filter(Boolean); break;
      case '--theme':
        if (!['light', 'dark', 'both'].includes(value)) usage('--theme은 light, dark, both 중 하나');
        opts.themes = value === 'both' ? ['light', 'dark'] : [value];
        break;
      case '--out': opts.out = resolve(value); break;
      default: usage(`알 수 없는 인자 ${arg}`);
    }
  }
  opts.viewports = opts.viewports.map((v) => {
    const m = /^(\d+)x(\d+)$/.exec(v);
    if (!m) usage(`뷰포트 ${v}는 <너비>x<높이> 꼴이어야 합니다`);
    return { width: Number(m[1]), height: Number(m[2]) };
  });
  return opts;
}

const slug = (name) => name.replace(/[^A-Za-z0-9]/g, '-').replace(/^-+|-+$/g, '') || 'home';
const vpName = (vp) => `${vp.width}x${vp.height}`;

// ---------- 서버·로그인 ----------
async function reachable(base) {
  try {
    await fetch(`${base}/login?fixture=login`, { signal: AbortSignal.timeout(4000), redirect: 'manual' });
    return true;
  } catch {
    return false;
  }
}

/** 폼을 직접 보내 로그인한다(로그인 화면에 기대지 않는다). 쿠키가 든 저장 상태를 돌려준다. */
async function login(base) {
  const api = await request.newContext({ baseURL: base });
  try {
    const response = await api.post('/login', {
      form: { password: process.env.LUCIA_E2E_PASSWORD ?? 'fixture-password-123' },
      headers: { Accept: 'application/json' },
      maxRedirects: 0,
    });
    const state = await api.storageState();
    if (!state.cookies.some((c) => c.name === 'lucia_admin')) {
      throw new Error(`로그인 응답에 lucia_admin 쿠키가 없습니다(HTTP ${response.status()}): ${(await response.text()).slice(0, 200)}`);
    }
    return state;
  } finally {
    await api.dispose();
  }
}

// ---------- 화면 하나 ----------
async function settle(page) {
  await page.evaluate(async () => {
    await document.fonts?.ready;
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  });
}

/** 앱이 그려질 때까지: #main이 생기고 네트워크가 잠잠하고 진행 막대가 사라질 때. 못 그리면 오류 글을 돌려준다. */
async function waitRendered(page) {
  try {
    await page.waitForSelector('#main, main', { state: 'attached', timeout: 15000 });
  } catch {
    return '#main이 15초 안에 생기지 않았습니다(화면을 그리지 못함)';
  }
  await page.waitForLoadState('networkidle', { timeout: 15000 }).catch(() => {});
  await page.waitForSelector('[role=progressbar]', { state: 'detached', timeout: 5000 }).catch(() => {});
  await settle(page);
  return null;
}

/**
 * 전체 화면 스크린샷. 고정 하단 막대가 긴 장 한가운데에 또 나오지 않게, 뷰포트 높이를 문서 높이에 맞춰 한 번에 찍고 되돌린다.
 */
async function screenshot(page, file, vp) {
  mkdirSync(dirname(file), { recursive: true });
  await page.evaluate(() => window.scrollTo(0, 0));
  let height = await page.evaluate(() => Math.ceil(document.documentElement.scrollHeight));
  let resized = false;
  if (height > vp.height && height <= SCREEN_LIMIT) {
    await page.setViewportSize({ width: vp.width, height });
    await settle(page);
    const again = await page.evaluate(() => Math.ceil(document.documentElement.scrollHeight));
    if (again > height && again <= SCREEN_LIMIT) {
      height = again;
      await page.setViewportSize({ width: vp.width, height });
      await settle(page);
    }
    resized = true;
  }
  await page.evaluate(() => window.scrollTo(0, 0));
  // 문서가 상한보다 길면 어쩔 수 없이 fullPage(고정 막대가 중간에 나올 수 있다).
  await page.screenshot({ path: file, fullPage: !resized });
  if (resized) await page.setViewportSize({ width: vp.width, height: vp.height });
}

async function checkPage(page, diag, ctx, entry) {
  const { opts, vp, touch, shotDir } = ctx;
  diag.begin({ url: entry.url, status: entry.status });
  const findings = [];
  const started = Date.now();
  try {
    await page.goto(opts.base + entry.url, { waitUntil: 'load', timeout: 20000 });
    const failure = await waitRendered(page);
    if (failure) {
      findings.push({ check: 'render', selector: '#main', text: failure, rect: null });
    } else {
      // 로그인 화면으로 튕겼거나 주소가 바뀌었다면 엉뚱한 화면을 검사한 것이다(깨끗해 보이는 거짓 통과를 막는다).
      const here = await page.evaluate(() => location.pathname + location.search);
      if (!entry.standalone && here !== entry.url) {
        findings.push({ check: 'render', selector: entry.url, text: `주소가 ${here}로 바뀌었습니다(로그인 화면으로 튕겼을 수 있음)`, rect: null });
      }
      const shot = opts.shots ? () => screenshot(page, join(shotDir, `${slug(entry.name)}.png`), vp) : null;
      findings.push(...(await runAllChecks(page, { touch, width: vp.width }, shot)));
    }
  } catch (error) {
    findings.push({ check: 'render', selector: entry.url, text: String(error.message ?? error).split('\n')[0].slice(0, 200), rect: null });
  }
  findings.push(...(await diag.end()));
  return { findings, ms: Date.now() - started };
}

// ---------- 보고 ----------
const rectText = (r) => (r ? `(${r.x},${r.y} ${r.w}x${r.h})` : '');
function lineOf(f) {
  const warn = isFailure(f) ? '' : '[경고] ';
  const where = f.selector ? ` ${f.selector}` : '';
  const text = f.text ? ` "${f.text}"` : '';
  const other = f.other ? ` <-> ${f.other.selector} "${f.other.text}"` : '';
  const detail = f.detail ? ` ${f.detail}` : '';
  const phase = f.phase ? ` {${f.phase}}` : '';
  const line = `${warn}${f.check}${where}${text}${other}${detail} ${rectText(f.rect)}${phase}`.trim();
  return line.length > 200 ? `${line.slice(0, 199)}…` : line;
}

function printReport(results, opts) {
  const LINES = 6; // 화면·뷰포트마다 보여 줄 줄 수(나머지는 report.json)
  const groups = new Map();
  for (const r of results) {
    const key = `${r.set} / ${r.theme}`;
    if (!groups.has(key)) groups.set(key, new Map());
    const pages = groups.get(key);
    if (!pages.has(r.page.name)) pages.set(r.page.name, []);
    pages.get(r.page.name).push(r);
  }
  for (const [key, pages] of groups) {
    console.log(`\n== ${key} ==`);
    let clean = 0;
    for (const [name, runs] of pages) {
      if (runs.every((r) => r.findings.length === 0)) {
        clean++;
        continue;
      }
      const failing = runs.some((r) => r.findings.some(isFailure));
      console.log(`${failing ? '실패' : '경고'}  ${name}  (${runs[0].page.url})`);
      for (const r of runs) {
        if (r.findings.length === 0) continue;
        const summary = Object.entries(r.findings.reduce((n, f) => ({ ...n, [f.check]: (n[f.check] ?? 0) + 1 }), {}))
          .map(([c, n]) => `${c} x${n}`)
          .join(', ');
        console.log(`   ${String(r.viewport.width).padStart(4)}px  ${summary}`);
        const ordered = r.findings.slice().sort((a, b) => Number(isFailure(b)) - Number(isFailure(a))); // 실패 먼저, 경고는 뒤
        for (const f of ordered.slice(0, LINES)) console.log(`          ${lineOf(f)}`);
        if (r.findings.length > LINES) console.log(`          … ${r.findings.length - LINES}건 더(report.json)`);
      }
    }
    console.log(`   문제 없는 화면 ${clean}개 / 전체 ${pages.size}개`);
  }
}

// ---------- 본 흐름 ----------
const opts = parseArgs(process.argv.slice(2));
const startedAt = Date.now();

if (!(await reachable(opts.base))) {
  console.error(
    `서버에 닿지 못했습니다: ${opts.base}\n` +
      '먼저 시험 데이터 서버를 띄웁니다(저장소 루트에서, 따로 둔 셸에서):\n' +
      '  LUCIA_UI_FIXTURE_ADDR=127.0.0.1:47890 cargo test -p luciad serve_ui_fixtures -- --ignored --nocapture\n' +
      '화면을 고쳤다면 먼저 (cd web/admin && npm run build). Host는 정확히 127.0.0.1:<포트>여야 합니다.',
  );
  process.exit(2);
}

const index = JSON.parse(readFileSync(join(here, '../fixtures/index.json'), 'utf8'));
const matches = (e) => !opts.only || e.name.includes(opts.only) || e.url.includes(opts.only);
const jobs = []; // { set, pages }
// --urls: 시험 데이터 목록 대신 준 주소만 본다(실제 데이터를 띄운 서버를 볼 때). 묶음 이름은 "urls"다.
if (opts.urls) {
  opts.sets = [];
  jobs.push({
    set: 'urls',
    pages: opts.urls.map((url) => ({ name: url.replace(/^\//, '') || 'home', url, status: 200 })),
  });
}
for (const set of opts.sets) {
  const pages = index.pages.filter((p) => p.set === set && matches(p));
  if (index.pages.every((p) => p.set !== set)) usage(`묶음 ${set}가 fixtures/index.json에 없습니다`);
  jobs.push({ set, pages });
}
if (opts.standalone) {
  const pages = index.standalone.filter(matches).map((s) => ({ ...s, status: 200, standalone: true }));
  jobs.push({ set: 'standalone', pages, standalone: true });
}
if (jobs.every((j) => j.pages.length === 0)) usage('조건에 맞는 화면이 없습니다(--only 확인)');

let storage;
try {
  storage = await login(opts.base);
} catch (error) {
  console.error(`로그인하지 못했습니다: ${error.message}`);
  process.exit(2);
}

let browserInfo;
try {
  browserInfo = await launchBrowser();
} catch (error) {
  console.error(error.message);
  process.exit(2);
}
const { browser, notes } = browserInfo;
for (const note of notes) console.error(`안내: ${note}`);
process.on('SIGINT', () => browser.close().finally(() => process.exit(130)));

const results = [];
let pagesChecked = 0;
try {
  for (const job of jobs) {
    // 세 줄 구조: 묶음 -> 판 -> 뷰포트 -> 화면. 뷰포트마다 맥락을 새로 만들고 끝나면 닫는다(메모리).
    // 로그인 없이 답하는 화면(standalone)은 첫 번째 판 하나만 본다.
    for (const theme of opts.themes) {
      if (job.standalone && theme !== opts.themes[0]) continue;
      for (const vp of opts.viewports) {
        if (job.pages.length === 0) continue;
        const touch = vp.width < 700;
        const context = await browser.newContext({
          storageState: storage,
          extraHTTPHeaders: { 'x-fixture-set': job.standalone ? 'full' : job.set },
          colorScheme: theme,
          // 바탕의 꽃잎(Petals)은 움직임 줄이기에서 그리지 않는다. 잴 때마다 자리가 달라지지 않게 기본은 줄임으로 잰다(--motion이면 켠 채로).
          reducedMotion: opts.motion ? 'no-preference' : 'reduce',
          viewport: vp,
          deviceScaleFactor: 1,
          locale: 'ko-KR',
          hasTouch: touch,
          isMobile: touch,
        });
        const t0 = Date.now();
        let bad = 0;
        try {
          await installProbes(context);
          const page = await context.newPage();
          const diag = watchConsole(page, opts.base);
          const shotDir = join(opts.out, 'shots', job.set, `${theme}-${vp.width}`);
          for (const entry of job.pages) {
            const { findings, ms } = await checkPage(page, diag, { opts, vp, touch, shotDir }, entry);
            results.push({
              set: job.set,
              theme,
              viewport: vp,
              page: { name: entry.name, url: entry.url, status: entry.status },
              ms,
              findings: findings.map((f) => ({ ...f, severity: isFailure(f) ? 'failure' : 'warning' })),
            });
            pagesChecked++;
            if (findings.some(isFailure)) bad++;
          }
        } finally {
          await context.close();
        }
        console.error(`${job.set} ${theme} ${vpName(vp)}: ${job.pages.length}개 화면, 실패 ${bad}개 (${((Date.now() - t0) / 1000).toFixed(1)}초)`);
      }
    }
  }
} finally {
  await browser.close();
}

const all = results.flatMap((r) => r.findings);
const failures = all.filter(isFailure);
const durationMs = Date.now() - startedAt;
mkdirSync(opts.out, { recursive: true });
writeFileSync(
  join(opts.out, 'report.json'),
  JSON.stringify(
    {
      meta: {
        base: opts.base,
        sets: opts.sets,
        themes: opts.themes,
        viewports: opts.viewports,
        only: opts.only,
        standalone: opts.standalone,
        shots: opts.shots,
        startedAt: new Date(startedAt).toISOString(),
        durationMs,
        notes,
        pageRuns: results.length,
        failures: failures.length,
        warnings: all.length - failures.length,
      },
      results,
    },
    null,
    2,
  ) + '\n',
);

printReport(results, opts);
console.log(
  `\n화면 ${pagesChecked}건(화면 x 뷰포트 x 판 x 묶음) 검사, 실패 ${failures.length}건, 경고 ${all.length - failures.length}건, ${(durationMs / 1000).toFixed(1)}초` +
    notes.map((n) => `\n주의: ${n}`).join('') +
    `\n보고서: ${join(opts.out, 'report.json')}${opts.shots ? `\n스크린샷: ${join(opts.out, 'shots')}` : ''}`,
);
process.exit(failures.length > 0 ? 1 : 0);
