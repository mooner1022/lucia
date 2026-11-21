// README의 그림을 만든다: 머리 그림(banner), 화면 모음(showcase), 프로필 그림(lucia.jpg).
// 관리 화면의 디자인 언어(docs/design.md)를 그대로 쓴다: 흰 옷감, 푸른 번짐, 쪽빛 띠와 금색 가는 줄, 꽃과 꽃잎.
//
//   web/admin/e2e/run.sh --sets healthy,fresh --viewports 1280x800,390x844 --theme both --shots --out <찍은 곳>
//   node web/admin/tools/gen-readme-assets.mjs --shots <찍은 곳>/shots [--avatar <프로필 그림>]
//
// - 화면 그림은 시험 데이터 서버에서 찍은 것만 쓴다(운영 화면을 찍지 않는다).
// - --avatar를 주지 않으면 이미 있는 docs/assets/readme/lucia.jpg를 쓴다.
// - 글꼴은 관리 화면이 쓰는 것(Pretendard, Gowun Batang)을 node_modules에서 읽는다.
// 결과: docs/assets/readme/{banner,showcase}-{light,dark}.png, lucia.jpg, lucia-round.png

import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { launchBrowser } from '../e2e/launch.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const admin = resolve(here, '..');
const out = resolve(admin, '../../docs/assets/readme');

const args = process.argv.slice(2);
const arg = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};
const shots = arg('--shots') ? resolve(arg('--shots')) : null;
const avatarSource = arg('--avatar') ? resolve(arg('--avatar')) : null;
if (!shots) {
  console.error('사용법: node tools/gen-readme-assets.mjs --shots <run.sh --shots의 shots 디렉터리> [--avatar <그림>]');
  process.exit(2);
}
mkdirSync(out, { recursive: true });

const url = (path) => pathToFileURL(path).href;
const dataUrl = (path, mime) => `data:${mime};base64,${readFileSync(path).toString('base64')}`;
const fonts = [
  'pretendard/dist/web/variable/pretendardvariable-dynamic-subset.css',
  '@fontsource/gowun-batang/400.css',
  '@fontsource/gowun-batang/700.css',
]
  .map((f) => `<link rel="stylesheet" href="${url(join(admin, 'node_modules', f))}">`)
  .join('');

// 문양(src/ui/motif와 같은 모양): 다섯 꽃잎의 꽃과 꽃잎 한 장.
const PETAL = 'M0 -1.5C-3.1 -2.1 -4.9 -4.9 -4.1 -7.7C-3.5 -9.8 -1.5 -10.5 0 -11.1C1.5 -10.5 3.5 -9.8 4.1 -7.7C4.9 -4.9 3.1 -2.1 0 -1.5Z';
const blossom = (fill, heart) =>
  `<g fill="${fill}">${[0, 72, 144, 216, 288].map((r) => `<path transform="rotate(${r})" d="${PETAL}"/>`).join('')}</g>` +
  (heart ? `<circle r="1.7" fill="${heart}"/>` : '');
const LEAF = 'M0 6C-3.6 4.6 -5.6 1.4 -4.6 -1.8C-3.8 -4.1 -1.7 -5 0 -6.2C1.7 -5 3.8 -4.1 4.6 -1.8C5.6 1.4 3.6 4.6 0 6Z';

// 판마다의 색(src/ui/tokens.css에서 옮긴 값).
const THEMES = {
  light: {
    bg: '#f3f5fc', sky: '#dfe7fb', wash: '#e6ecfc', hem: '#c4d3f6', glow: '#ffffff', surface: '#ffffff',
    text: '#1c2143', text2: '#424a70', border: '#d8def2', drift: '#9db0ec', driftAlpha: 0.6, petal: '#ffffff',
    petalAlpha: 0.85, band: '#2f3f96', band2: '#4055b4', line: '#ecd9ae', gold: '#b89454', moon: 0.9,
    shadow: '0 18px 50px rgba(47, 63, 150, 0.18)',
  },
  dark: {
    bg: '#0c1024', sky: '#171f4b', wash: '#171d42', hem: '#222c66', glow: '#4a5cc0', surface: '#141936',
    text: '#eef0ff', text2: '#b9c0e6', border: '#2a3263', drift: '#aebcf5', driftAlpha: 0.32, petal: '#aebcf5',
    petalAlpha: 0.22, band: '#33439f', band2: '#4458bd', line: '#ecd9ae', gold: '#c9a868', moon: 0.92,
    shadow: '0 18px 50px rgba(0, 0, 0, 0.5)',
  },
};

// 늘 같은 그림이 나오게 정해 둔 자리(가로 %, 세로 %, 크기 px, 도는 각, 진하기).
const DRIFT = [
  [6, 14, 16, -30, 0.9], [15, 62, 12, 40, 0.7], [24, 30, 10, 110, 0.6], [33, 78, 14, -70, 0.8], [41, 10, 9, 20, 0.5],
  [52, 44, 11, 150, 0.45], [58, 84, 15, -15, 0.8], [66, 20, 10, 75, 0.6], [73, 58, 13, -120, 0.75], [81, 34, 9, 35, 0.5],
  [88, 72, 16, -50, 0.9], [94, 16, 11, 95, 0.6], [47, 66, 8, -95, 0.4], [70, 90, 10, 60, 0.6], [28, 92, 9, 10, 0.5],
];
const HEM = [
  [3, 78, 58, 12], [11, 92, 40, -20], [19, 70, 30, 35], [27, 96, 34, 5], [36, 84, 22, -30], [58, 94, 26, 18],
  [69, 82, 36, -12], [78, 96, 30, 28], [86, 74, 46, -6], [95, 90, 60, 20], [47, 98, 20, 0],
];

function decorations(t) {
  const drift = DRIFT.map(
    ([x, y, s, r, a]) =>
      `<svg style="left:${x}%;top:${y}%;width:${s}px;height:${s * 1.15}px;opacity:${(t.driftAlpha * a).toFixed(2)};transform:rotate(${r}deg)" viewBox="-6 -7 12 14"><path fill="${t.drift}" d="${LEAF}"/></svg>`,
  ).join('');
  const hem = HEM.map(
    ([x, y, s, r]) =>
      `<svg style="left:${x}%;top:${y}%;width:${s}px;height:${s}px;margin:-${s / 2}px 0 0 -${s / 2}px;opacity:${t.petalAlpha};transform:rotate(${r}deg)" viewBox="-12 -12 24 24">${blossom(t.petal)}</svg>`,
  ).join('');
  return `<div class="deco">${drift}${hem}</div>`;
}

const base = (t) => `
  * { box-sizing: border-box; margin: 0; }
  html, body { background: ${t.bg}; }
  body { font-family: 'Pretendard Variable', Pretendard, sans-serif; color: ${t.text}; -webkit-font-smoothing: antialiased; }
  .stage { position: relative; overflow: hidden;
    background:
      radial-gradient(60rem 26rem at 82% -30%, ${t.glow}${t === THEMES.light ? '' : '55'}, transparent 70%),
      ${t === THEMES.light
        ? `linear-gradient(180deg, ${t.sky} 0%, ${t.bg} 46%, ${t.wash} 74%, ${t.hem} 100%)`
        : `linear-gradient(180deg, #151c47 0%, #10163a 52%, #1d2659 100%)`}; }
  .deco { position: absolute; inset: 0; pointer-events: none; }
  .deco svg { position: absolute; }
  .serif { font-family: 'Gowun Batang', serif; }
`;

function banner(t, avatar) {
  return `<!doctype html><meta charset="utf-8">${fonts}<style>${base(t)}
    .stage { width: 1280px; height: 400px; display: flex; align-items: center; gap: 56px; padding: 0 96px; }
    .moon { position: absolute; right: 150px; top: 40px; width: 128px; height: 128px; border-radius: 50%;
      background: radial-gradient(circle at 40% 36%, #ffffff 0%, #f1f4ff 52%, ${t === THEMES.light ? '#dde5fb' : '#c6d0ff'} 100%);
      opacity: ${t.moon};
      box-shadow: ${t === THEMES.light ? '0 0 0 1px #d3dcf5, 0 0 60px 22px #ffffff' : '0 0 90px 30px rgba(140, 160, 255, 0.45)'}; }
    .face { position: relative; flex: none; width: 232px; height: 232px; border-radius: 50%; padding: 7px;
      background: ${t.surface}; box-shadow: 0 0 0 1.5px ${t.gold}, ${t.shadow}; }
    .face img { display: block; width: 100%; height: 100%; border-radius: 50%; object-fit: cover; }
    .mark { position: absolute; right: 2px; bottom: 6px; width: 60px; height: 60px; border-radius: 50%;
      background: linear-gradient(135deg, #2f3f96, #4a60c0); box-shadow: 0 0 0 4px ${t.surface}, 0 6px 16px rgba(20, 28, 80, 0.35);
      display: grid; place-items: center; }
    .mark svg { width: 38px; height: 38px; margin-top: 3px; }
    .words { position: relative; display: flex; flex-direction: column; align-items: flex-start; gap: 18px; }
    h1 { font: 700 92px/1 'Gowun Batang', serif; letter-spacing: -0.01em; }
    .band { position: relative; padding: 11px 26px 11px 24px; border-radius: 999px; color: #fff; font-weight: 600; font-size: 22px;
      background: linear-gradient(120deg, ${t.band}, ${t.band2}); box-shadow: 0 8px 22px rgba(47, 63, 150, 0.28); }
    .band::after { content: ''; position: absolute; inset: 4px; border-radius: inherit; border: 1px solid ${t.line}; opacity: 0.55; }
    .motto { font: 400 21px/1.5 'Gowun Batang', serif; color: ${t.text2}; letter-spacing: 0.01em; }
    .rule { display: flex; align-items: center; gap: 10px; width: 420px; }
    .rule svg { width: 18px; height: 18px; flex: none; }
    .rule i { flex: 1; height: 1px; background: linear-gradient(90deg, ${t.gold}, transparent); }
  </style>
  <div class="stage">${decorations(t)}<div class="moon"></div>
    <div class="face"><img src="${avatar}" alt="">
      <div class="mark"><svg viewBox="-12 -12 24 24">${blossom('#fff')}</svg></div></div>
    <div class="words">
      <h1>Lucia</h1>
      <div class="rule"><svg viewBox="-12 -12 24 24">${blossom(t === THEMES.light ? t.band : '#a3b4ff')}</svg><i></i></div>
      <div class="band">홈서버에서 늘 켜 두는 한 사람용 개인 비서 에이전트</div>
      <p class="motto">구름처럼, 조용히 흐르고 달빛처럼, 은은히 비추며…</p>
    </div>
  </div>`;
}

function showcase(t, desktop, phone) {
  return `<!doctype html><meta charset="utf-8">${fonts}<style>${base(t)}
    .stage { width: 1280px; height: 760px; }
    .desk { position: absolute; left: 70px; top: 56px; width: 960px; height: 620px; border-radius: 16px; overflow: hidden;
      border: 1px solid ${t.border}; box-shadow: ${t.shadow}; background: ${t.surface}; }
    .desk img { display: block; width: 100%; }
    .phone { position: absolute; right: 70px; top: 150px; width: 270px; height: 560px; border-radius: 34px; overflow: hidden;
      border: 7px solid ${t === THEMES.light ? '#1c2143' : '#05071a'}; box-shadow: ${t.shadow}, 0 0 0 1.5px ${t.gold};
      background: ${t.surface}; }
    .phone img { display: block; width: 100%; }
  </style>
  <div class="stage">${decorations(t)}
    <div class="desk"><img src="${desktop}" alt=""></div>
    <div class="phone"><img src="${phone}" alt=""></div>
  </div>`;
}

const { browser, notes } = await launchBrowser();
for (const note of notes) console.warn(note);
const page = await browser.newPage({ deviceScaleFactor: 2 });

// 파일로 두고 연다: 빈 페이지에 글로 넣으면 node_modules의 글꼴 파일(file://)을 읽지 못한다.
const work = mkdtempSync(join(tmpdir(), 'lucia-readme-'));
async function shoot(html, file, width, height, options = {}) {
  await page.setViewportSize({ width, height });
  const source = join(work, 'page.html');
  writeFileSync(source, html);
  await page.goto(url(source), { waitUntil: 'load' });
  await page.evaluate(async () => {
    await Promise.all([...document.fonts].map((f) => f.load().catch(() => null)));
    await document.fonts.ready;
  });
  await page.screenshot({ path: join(out, file), ...options });
  console.log(file);
}

// 프로필 그림: 줄여서 JPEG로 둔다(원본의 메타데이터는 따라오지 않는다).
const avatarFile = join(out, 'lucia.jpg');
if (avatarSource) {
  const mime = /\.jpe?g$/i.test(avatarSource) ? 'image/jpeg' : 'image/png';
  await shoot(
    `<!doctype html><style>*{margin:0}img{display:block;width:320px;height:320px;object-fit:cover}</style><img src="${dataUrl(avatarSource, mime)}">`,
    'lucia.jpg',
    320,
    320,
    { type: 'jpeg', quality: 90 },
  );
} else if (!existsSync(avatarFile)) {
  console.error(`프로필 그림이 없습니다: --avatar <그림>을 주거나 ${avatarFile}을 둔다`);
  process.exit(2);
}
const avatar = dataUrl(avatarFile, 'image/jpeg');

// 둥근 프로필 그림(바탕이 비치는 PNG): 흰 테와 금색 가는 줄. 밝은 판과 어두운 판에 같이 쓴다.
await shoot(
  `<!doctype html><style>*{margin:0}html,body{background:transparent}
    div{width:240px;height:240px;padding:10px}
    img{display:block;width:100%;height:100%;border-radius:50%;object-fit:cover;
      box-shadow:0 0 0 5px #fff,0 0 0 6.5px #b89454}</style><div><img src="${avatar}"></div>`,
  'lucia-round.png',
  240,
  240,
  { omitBackground: true },
);

for (const [name, t] of Object.entries(THEMES)) {
  await shoot(banner(t, avatar), `banner-${name}.png`, 1280, 400);
  const desktop = join(shots, 'healthy', `${name}-1280`, 'home.png');
  const phone = join(shots, 'fresh', `${name}-390`, 'start.png');
  for (const f of [desktop, phone]) {
    if (!existsSync(f)) {
      console.error(`화면 그림이 없습니다: ${f}\n먼저: web/admin/e2e/run.sh --sets healthy,fresh --viewports 1280x800,390x844 --theme both --shots --out <찍은 곳>`);
      process.exit(2);
    }
  }
  await shoot(showcase(t, dataUrl(desktop, 'image/png'), dataUrl(phone, 'image/png')), `showcase-${name}.png`, 1280, 760);
}
await browser.close();
rmSync(work, { recursive: true, force: true });
