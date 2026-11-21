// 기능 보존(결정 0041·0061). 화면을 Svelte로 옮기면서 **쓸 수 있던 것이 하나도 사라지지 않았는지** 본다.
//
// 개편 전 화면에서 뽑아 둔 기준 목록(tests/parity_baseline.txt, 결정 0041에서 만든 그대로)의 모든 줄이 새 화면의
// DOM에 있어야 한다: 폼(주소·방식), 입력(이름·종류·한도 속성·값), 고르기 값, 링크, 보이던 데이터의 표지(`qz-…`).
// 화면 하나씩도 맞춘다(fixtures/baseline-by-page.json): 그 화면에 있던 것은 그 화면에 있어야 한다.
//
// 한 화면만 돌리기: npx vitest run tests/parity.test.ts -t "/connections"
import { afterEach, describe, expect, it } from 'vitest';
import { SETS, baseline, baselineByPage, clear, inventory, pages, render, screens } from './helpers';
import { MOVED } from './parity-moves';

/** 아직 옮기지 않은 화면의 자리 표시 글. 이 글이 보이는 화면은 건너뛴다(다 옮기면 건너뛰는 것이 없어야 한다). */
const PLACEHOLDER = '아직 옮기는 중입니다';

/**
 * 새 화면에서 다른 것으로 바뀐 줄(의도한 대체, 결정 0061에 까닭을 적었다).
 * - refresh 3: 화면 넘겨받기의 자동 새로 고침. `<meta http-equiv=refresh>` 대신 화면이 3초마다 다시 묻는다(tests/pages.test.ts가 본다).
 * - 나머지: 입력 칸의 HTML id였던 글(데이터가 아니다). 새 화면의 id는 부품이 만든다.
 */
const REPLACED = new Set([
  'refresh 3',
  'data qz-client-a-help',
  'data qz-w-items-fetcher',
  'data qz-w-items-fetcher-help',
  'data qz-w-items-interval',
  'data qz-w-items-notify',
  'data qz-w-login-fetcher',
  'data qz-w-login-fetcher-help',
  'data qz-w-login-interval',
  'data qz-w-login-notify',
  'data qz-w-region-interval',
]);

afterEach(clear);

/** 이름이 같은 화면을 모든 상태 묶음으로 그려 줄을 모은다. 옮기지 않은 화면이면 null. */
function collect(name: string): Set<string> | null {
  const got = new Set<string>();
  let drawn = 0;
  for (const set of SETS) {
    for (const p of pages(set).filter((p) => p.name === name)) {
      const body = render(p.data, p.url);
      if (body.textContent?.includes(PLACEHOLDER)) return null;
      for (const line of inventory(body)) got.add(line);
      drawn++;
    }
  }
  for (const s of screens().filter((s) => s.name === name)) {
    const body = render(s.data, s.url);
    if (body.textContent?.includes(PLACEHOLDER)) return null;
    for (const line of inventory(body)) got.add(line);
    drawn++;
  }
  if (drawn === 0) throw new Error(`시험 데이터에 없는 화면: ${name}`);
  return got;
}

describe('화면마다: 개편 전에 있던 것이 그대로 있다', () => {
  for (const [name, lines] of Object.entries(baselineByPage())) {
    it(name, (ctx) => {
      const got = collect(name);
      if (!got) return ctx.skip();
      const moved = new Set(MOVED[name] ?? []);
      const missing = lines.filter((l) => !got.has(l) && !REPLACED.has(l) && !moved.has(l));
      expect(missing, `${name}: 사라진 것 ${missing.length}개`).toEqual([]);
    });
  }
});

it('전체: 기준 목록의 모든 줄이 어느 화면엔가 있다', (ctx) => {
  const got = new Set<string>();
  const names = new Set<string>([...SETS.flatMap((s) => pages(s).map((p) => p.name)), ...screens().map((s) => s.name)]);
  for (const name of names) {
    const lines = collect(name);
    if (!lines) return ctx.skip();
    for (const line of lines) got.add(line);
  }
  const missing = baseline().filter((l) => !got.has(l) && !REPLACED.has(l));
  expect(missing, `사라진 기능 ${missing.length}개`).toEqual([]);
});

it('다른 화면으로 옮긴 것은 실제로 그 화면에 있다', (ctx) => {
  for (const [from, lines] of Object.entries(MOVED)) {
    for (const line of lines) {
      const to = MOVED_TO[`${from}\n${line}`];
      expect(to, `${from}의 "${line}"이 어디로 갔는지 parity-moves.ts에 적는다`).toBeTruthy();
      const got = collect(to);
      if (!got) return ctx.skip();
      expect(got.has(line), `${to}에 "${line}"`).toBe(true);
    }
  }
});

import { MOVED_TO } from './parity-moves';
