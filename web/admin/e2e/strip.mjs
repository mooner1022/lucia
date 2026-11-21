// 긴 세로 스크린샷을 여러 조각으로 잘라 옆으로 붙인다(한 장으로 훑어보기 좋게).
//   node e2e/strip.mjs <입력.png> <출력.png> [조각 높이(px), 기본 1500]
import { readFileSync } from 'node:fs';
import { launchBrowser } from './launch.mjs';

const [input, output, heightArg] = process.argv.slice(2);
if (!input || !output) {
  console.error('쓰기: node e2e/strip.mjs <입력.png> <출력.png> [조각 높이]');
  process.exit(2);
}
const sliceHeight = Number(heightArg ?? 1500);
const data = readFileSync(input).toString('base64');
const { browser } = await launchBrowser();
try {
  const page = await browser.newPage({ deviceScaleFactor: 1 });
  await page.setContent(`<body style="margin:0;background:#888"><img id="src" src="data:image/png;base64,${data}"></body>`);
  const size = await page.evaluate(async () => {
    const img = document.getElementById('src');
    await img.decode();
    return { w: img.naturalWidth, h: img.naturalHeight };
  });
  const parts = Math.ceil(size.h / sliceHeight);
  const gap = 12;
  await page.evaluate(
    ({ size, parts, sliceHeight, gap }) => {
      const src = document.getElementById('src').src;
      document.body.innerHTML = '';
      const row = document.createElement('div');
      row.style.cssText = `display:flex;gap:${gap}px;align-items:flex-start;width:max-content`;
      for (let i = 0; i < parts; i++) {
        const cell = document.createElement('div');
        cell.style.cssText = `width:${size.w}px;height:${Math.min(sliceHeight, size.h - i * sliceHeight)}px;background:url(${src}) 0 ${-i * sliceHeight}px no-repeat`;
        row.append(cell);
      }
      document.body.append(row);
    },
    { size, parts, sliceHeight, gap },
  );
  await page.setViewportSize({ width: parts * size.w + (parts - 1) * gap, height: Math.min(sliceHeight, size.h) });
  await page.screenshot({ path: output });
  console.log(`${size.w}x${size.h} → ${parts}조각`);
} finally {
  await browser.close();
}
