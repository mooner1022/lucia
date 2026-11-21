// 표기 함수가 데몬과 같은 글을 내는지(fixtures/format-cases.json은 데몬이 만든다).
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, it } from 'vitest';
import { bytes, exact, num, usd, when } from '../src/lib/format';
import { ROOT } from './helpers';

const cases = JSON.parse(readFileSync(join(ROOT, 'fixtures/format-cases.json'), 'utf8')) as {
  times: { at: string; now: string; tz: string; when: string; exact: string }[];
  num: [number, string][];
  usd: [number, string][];
  bytes: [number, string][];
};

it('시각 표기', () => {
  for (const c of cases.times) {
    expect(when(c.at, c.now, c.tz), `when ${c.at} @ ${c.tz}`).toBe(c.when);
    expect(exact(c.at, c.tz), `exact ${c.at} @ ${c.tz}`).toBe(c.exact);
  }
});

it('숫자 표기', () => {
  for (const [n, text] of cases.num) expect(num(n)).toBe(text);
  for (const [v, text] of cases.usd) expect(usd(v)).toBe(text);
  for (const [n, text] of cases.bytes) expect(bytes(n)).toBe(text);
});
