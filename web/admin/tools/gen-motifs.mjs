// 문양 그림을 만든다(결정 0062): 꽃(다섯 꽃잎), 꽃잎 한 장(흩날리는 꽃잎), 치맛단 꽃무늬.
//   node tools/gen-motifs.mjs      → src/ui/motif/{blossom,petal,hem}.svg
// 그림은 색이 없는 모양뿐이다. 부품이 mask-image로 걸고 색은 토큰(background-color)으로 칠한다(밝은·어두운 판을 따른다).
// 모양을 바꾸려면 아래 숫자를 고치고 다시 만든 뒤, 크게 그려서 눈으로 본다(web/admin/docs/design.md "문양").
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const out = join(dirname(fileURLToPath(import.meta.url)), '../src/ui/motif');
mkdirSync(out, { recursive: true });
// 꽃: 끝이 살짝 뾰족한 둥근 꽃잎 다섯 장(가운데는 비워 별 모양 꽃술 자리가 남는다).
const PETAL = 'M0 -1.5C-3.1 -2.1 -4.9 -4.9 -4.1 -7.7C-3.5 -9.8 -1.5 -10.5 0 -11.1C1.5 -10.5 3.5 -9.8 4.1 -7.7C4.9 -4.9 3.1 -2.1 0 -1.5Z';
const flower = (x, y, scale, rotate = 0) =>
  `<g transform="translate(${x} ${y}) rotate(${rotate}) scale(${scale})">${[0, 72, 144, 216, 288].map((a) => `<path transform="rotate(${a})" d="${PETAL}"/>`).join('')}</g>`;
// 꽃잎 하나가 위를 향해 놓여 꽃의 위아래가 고르지 않다(위 11.1, 아래 9). 눈에 가운데로 보이게 CENTER만큼 내려 그린다.
// (아이콘 `blossom`(src/ui/icons.ts)과 파비콘(public/favicon.svg)도 같은 만큼 내려 있다.)
const CENTER = 0.8;
writeFileSync(join(out, 'blossom.svg'), `<svg xmlns="http://www.w3.org/2000/svg" viewBox="-12 -12 24 24" fill="#000">${flower(0, CENTER, 1)}</svg>\n`);

// 꽃잎 한 장: 밑동과 끝이 뾰족한 잎 모양(바탕에 흩날린다).
writeFileSync(
  join(out, 'petal.svg'),
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="-6 -7 12 14" fill="#000"><path d="M0 6C-3.6 4.6 -5.6 1.4 -4.6 -1.8C-3.8 -4.1 -1.7 -5 0 -6.2C1.7 -5 3.8 -4.1 4.6 -1.8C5.6 1.4 3.6 4.6 0 6Z"/></svg>\n',
);

// 치맛단 꽃무늬: 아래로 갈수록 크고 촘촘하다(치마의 푸른 번짐 위에 흰 꽃).
// 옆으로 이어 붙여 쓰기도 하므로(Solo) 어느 꽃도 좌우 가장자리에 걸치지 않게 둔다.
const HEM = [
  [34, 168, 2.3, 10], [96, 150, 1.5, 40], [150, 176, 2.0, -15], [206, 152, 1.7, 25], [70, 112, 1.1, 5], [176, 110, 1.2, 50],
  [222, 184, 1.2, 0], [16, 122, 0.9, 30], [128, 78, 0.8, 20], [40, 60, 0.6, 12], [200, 52, 0.6, 44], [100, 30, 0.45, 0],
];
const DOTS = [[60, 140], [120, 128], [186, 134], [220, 100], [20, 90], [150, 46], [84, 70]];
writeFileSync(
  join(out, 'hem.svg'),
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 240 200" fill="#000">${HEM.map((h) => flower(...h)).join('')}${DOTS.map(([x, y]) => `<circle cx="${x}" cy="${y}" r="1.4"/>`).join('')}</svg>\n`,
);
console.log(`문양 3개를 ${out}에 썼다`);
