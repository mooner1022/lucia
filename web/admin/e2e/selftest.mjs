// 검출기 자체 시험: 일부러 깨뜨린 작은 HTML을 실제 브라우저에 올려, 검사(checks.mjs)가 정확히 그 문제만 잡는지 본다.
// 서버가 필요 없다(page.setContent). 하나라도 어긋나면 종료 코드 1.
//
//   cd web/admin && export PATH=~/.local/opt/node/bin:$PATH && node e2e/selftest.mjs
//
// 검사가 "아무것도 못 잡아서 늘 통과하는" 상태가 되지 않게 하려는 것이다. 맞게 만든 화면(clean·scroll-wrapper 등)은
// 아무것도 안 나와야 하고, 깨진 화면은 정해진 검사만 나와야 한다(너무 많이 잡아도 실패).

import { runAllChecks, runChecks } from './checks.mjs';
import { launchBrowser } from './launch.mjs';

const BASE_CSS = `
  * { box-sizing: border-box; }
  body { margin: 0; font: 16px/1.4 'DejaVu Sans', sans-serif; }
  .sr-only { position: absolute; width: 1px; height: 1px; margin: -1px; padding: 0; overflow: hidden; clip-path: inset(50%); white-space: nowrap; border: 0; }
`;
const doc = (body) =>
  `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><style>${BASE_CSS}</style></head><body>${body}</body></html>`;

const rows = (n) => Array.from({ length: n }, (_, i) => `<p style="margin:0">Scrolling row number ${i + 1} of the long list</p>`).join('');

const cases = [
  {
    name: '맞게 만든 화면(조각 모음)은 아무것도 안 나온다',
    expect: [],
    html: doc(`
      <header style="position:sticky;top:0;background:#fff;padding:8px"><a href="#">Home</a> <a href="#">Docs</a> <button>Menu</button></header>
      <main style="padding:16px">
        <h1 style="margin:0 0 8px">Title of the page</h1>
        <p>Paragraph that wraps over a few lines in a narrow viewport without any trouble at all, really.</p>
        <div style="display:grid;grid-template-columns:1fr 1fr;gap:12px">
          <div><h2 style="margin:0">One</h2><p style="overflow-wrap:anywhere">https://example.test/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</p></div>
          <div><h2 style="margin:0">Two</h2><p>Short text.</p></div>
        </div>
        <form style="display:flex;flex-wrap:wrap;gap:8px"><label for="n">Name</label><input id="n"><label><input type="checkbox"> Check me</label><button>Save</button></form>
        <p class="sr-only">Screen reader only text laid over everything</p>
        <details><summary>More</summary><p>Hidden body text that is fine.</p></details>
      </main>`),
  },
  {
    name: '맞게 쓴 가로 스크롤 상자 안의 넓은 표는 문제가 아니다',
    expect: [],
    html: doc(`<div style="overflow-x:auto;padding:8px"><table style="width:1200px;border-collapse:collapse"><tr><td>Cell one</td><td>Cell two</td><td>Cell three</td></tr></table></div>`),
  },
  {
    name: '말줄임표(text-overflow: ellipsis)는 의도된 줄임이라 잘림이 아니다',
    expect: [],
    html: doc(`<div style="width:80px;overflow:hidden;white-space:nowrap;text-overflow:ellipsis">A very long label text that gets an ellipsis</div>`),
  },
  {
    name: '보이지 않는 글끼리의 겹침은 무시한다(display·visibility·opacity·sr-only·잘려 나간 글·닫힌 dialog)',
    expect: [],
    html: doc(`
      <div style="position:relative;height:40px">
        <div>Visible line of text</div>
        <div style="position:absolute;top:0;left:0;display:none">ghost display none</div>
        <div style="position:absolute;top:0;left:0;visibility:hidden">ghost visibility hidden</div>
        <div style="position:absolute;top:0;left:0;opacity:0">ghost opacity zero</div>
        <div class="sr-only" style="top:0;left:0">ghost sr-only text</div>
        <div style="position:absolute;top:0;left:0;width:200px;height:0;overflow:hidden">ghost clipped away</div>
        <dialog style="position:absolute;top:0;left:0">ghost closed dialog</dialog>
      </div>`),
  },
  {
    name: '위 시험의 짝: 같은 자리의 글이 보이면(opacity 1) 겹침으로 잡힌다',
    expect: ['overlap'],
    html: doc(`<div style="position:relative;height:40px"><div>Visible line of text</div><div style="position:absolute;top:0;left:0;opacity:1">ghost now visible</div></div>`),
  },
  {
    name: '위 시험의 짝: 잘라 주는 상자에 높이가 있어 글이 보이면 겹침으로 잡힌다',
    expect: ['overlap'],
    html: doc(`<div style="position:relative;height:40px"><div>Visible line of text</div><div style="position:absolute;top:0;left:0;width:200px;height:20px;overflow:hidden">ghost partly visible</div></div>`),
  },
  {
    name: '위 시험의 짝: 위치 기준이 바깥인 절대 위치 글은 높이 0 상자 안에 있어도 잘리지 않으니 겹친다',
    expect: ['overlap'],
    html: doc(`<div style="position:relative;height:40px"><div>Visible line of text</div><div style="height:0;overflow:hidden"><div style="position:absolute;top:0;left:0">ghost escapes the clip</div></div></div>`),
  },
  {
    name: '음수 여백으로 글이 겹친다 -> overlap',
    expect: ['overlap'],
    html: doc(`<div>First block of text</div><div style="margin-top:-14px">Second block drawn on top</div>`),
  },
  {
    name: '줄이 바뀌는 상자에 고정 height를 주면 글이 넘쳐 다음 글을 덮는다 -> overlap',
    expect: ['overlap'],
    html: doc(`<div style="width:90px;height:22px;background:#eee">This text wraps into many lines and spills</div><div>Next block of text</div>`),
  },
  {
    name: '1fr 칸에 끊을 곳 없는 긴 낱말(min-width: 0 없음)이 들어가 가로로 밀린다 -> overflow-x, outside',
    expect: ['outside', 'overflow-x'],
    html: doc(`<div style="display:grid;grid-template-columns:1fr 1fr;gap:8px;padding:8px">
      <div><h3 style="margin:0">Label</h3><p style="margin:0">https://example.test/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</p></div>
      <div><h3 style="margin:0">Other</h3></div></div>`),
  },
  {
    name: '같은 격자 깨짐이 모바일(터치 에뮬레이션)에서도 잡힌다 -> overflow-x, outside',
    touch: true,
    width: 360,
    height: 640,
    expect: ['outside', 'overflow-x'],
    html: doc(`<div style="display:grid;grid-template-columns:1fr 1fr;gap:8px;padding:8px">
      <div><h3 style="margin:0">Label</h3><p style="margin:0">https://example.test/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</p></div>
      <div><h3 style="margin:0">Other</h3></div></div>`),
  },
  {
    name: '절대 위치 배지가 제목을 덮는다 -> overlap',
    expect: ['overlap'],
    html: doc(`<div style="position:relative;padding:20px"><h2 style="margin:0">Section heading text</h2><span style="position:absolute;top:20px;left:40px;background:#fc0">NEW BADGE</span></div>`),
  },
  {
    name: '단추가 링크를 덮는다 -> control-overlap',
    expect: ['control-overlap'],
    html: doc(`<div style="padding:20px"><a href="#" style="display:inline-block;width:120px;padding:6px 0">Link</a><button style="margin-left:-60px;width:140px">Button label here</button></div>`),
  },
  {
    name: 'nowrap + overflow: hidden 라벨이 글을 자른다 -> clipped',
    expect: ['clipped'],
    html: doc(`<div style="width:80px;overflow:hidden;white-space:nowrap">A very long label text that is cut</div>`),
  },
  {
    name: '펼쳐야만 나오는 깨짐: 닫힌 details 속의 겹침은 펼친 뒤에야 잡힌다(phase: details-open)',
    html: doc(`<details><summary>More</summary><div>First block inside</div><div style="margin-top:-14px">Second block inside</div></details>`),
    async run(page, options) {
      const before = await runChecks(page, options);
      if (before.length > 0) return [{ check: `펼치기 전에 이미 나옴:${before.map((f) => f.check)}` }];
      const after = await runAllChecks(page, options);
      return after.filter((f) => f.phase !== 'details-open').length === 0 ? after : [{ check: '펼치기 전 발견이 섞임' }, ...after];
    },
    expect: ['overlap'],
    expectPhase: 'details-open',
  },
  {
    name: '스크롤하는 글 위에 뜬 붙는 머리글·고정 막대는 겹침으로 치지 않는다',
    html: doc(`<header style="position:sticky;top:0;background:#fff;height:40px;padding:8px">Sticky header</header>${rows(60)}
      <div style="position:fixed;left:0;right:0;bottom:0;height:40px;padding:8px;background:#fff">Fixed bottom bar</div>`),
    async run(page, options) {
      await page.evaluate(() => window.scrollTo(0, 200));
      return runChecks(page, options);
    },
    expect: [],
  },
  {
    name: '같은 화면을 ignoreFloating:false로 보면 겹침이 잡힌다(위 시험이 우연히 통과한 게 아님을 보인다)',
    html: doc(`<header style="position:sticky;top:0;background:#fff;height:40px;padding:8px">Sticky header</header>${rows(60)}`),
    async run(page, options) {
      await page.evaluate(() => window.scrollTo(0, 200));
      return runChecks(page, { ...options, ignoreFloating: false });
    },
    expect: ['overlap'],
  },
  {
    name: '터치 화면: 16x16 단추는 tiny-target 경고만(정상 크기 단추는 문제없음)',
    touch: true,
    width: 360,
    height: 640,
    expect: ['tiny-target'],
    html: doc(`<div style="padding:16px;display:flex;gap:16px"><button style="width:16px;height:16px;padding:0">x</button><button style="width:44px;height:44px">ok</button></div>`),
  },
  {
    name: '데스크톱 화면에서는 tiny-target을 보지 않는다',
    expect: [],
    html: doc(`<div style="padding:16px"><button style="width:16px;height:16px;padding:0">x</button></div>`),
  },
];

const { browser, notes } = await launchBrowser();
for (const note of notes) console.log(`안내: ${note}`);
let failed = 0;
try {
  for (const c of cases) {
    const touch = Boolean(c.touch);
    const width = c.width ?? 400;
    const height = c.height ?? 600;
    const context = await browser.newContext({
      viewport: { width, height },
      deviceScaleFactor: 1,
      locale: 'ko-KR',
      hasTouch: touch,
      isMobile: touch,
    });
    try {
      const page = await context.newPage();
      await page.setContent(c.html);
      await page.evaluate(() => document.fonts.ready);
      const options = { touch, width };
      const findings = c.run ? await c.run(page, options) : await runAllChecks(page, options);
      const got = [...new Set(findings.map((f) => f.check))].sort();
      const want = [...c.expect].sort();
      let ok = got.length === want.length && got.every((v, i) => v === want[i]);
      if (ok && c.expectPhase) ok = findings.length > 0 && findings.every((f) => f.phase === c.expectPhase);
      console.log(`${ok ? '통과' : '실패'}  ${c.name}  [${got.join(', ') || '없음'}]`);
      if (!ok) {
        failed++;
        console.log(`      기대: [${want.join(', ') || '없음'}]${c.expectPhase ? ` phase=${c.expectPhase}` : ''}`);
        for (const f of findings.slice(0, 6)) console.log(`      ${f.check} ${f.selector ?? ''} "${f.text ?? ''}" ${f.phase ?? ''} ${JSON.stringify(f.rect)}`);
      }
    } finally {
      await context.close();
    }
  }
} finally {
  await browser.close();
}
console.log(failed === 0 ? `\n자체 시험 ${cases.length}개 모두 통과` : `\n자체 시험 ${failed}개 실패 (전체 ${cases.length}개)`);
process.exit(failed === 0 ? 0 : 1);
