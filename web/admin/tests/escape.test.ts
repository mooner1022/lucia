// 주입 방어(결정 0061). 화면에 찍히는 바깥 글(메일 제목, 웹 결과, 다른 사람의 글, 처리 결과 안내)이 마크업으로
// 들어가지 않는지 본다. 예전에는 데몬의 템플릿이 이스케이프했고 데몬 시험이 그것을 봤다. 이제 그리는 것이 화면이므로
// 여기서 본다: 시험 데이터의 모든 글에 태그를 심어 모든 화면을 그리고, 그 태그가 요소로 생기지 않았는지 확인한다.
// (CSP가 인라인 스크립트를 막지만, 그것은 두 번째 겹이다.)
import { afterEach, describe, expect, it } from 'vitest';
import type { PageData, StandaloneData } from '../src/lib/types';
import { clear, pages, render, screens } from './helpers';

const BAIT = '<i data-bait="1">x</i><img src="x" data-bait="1"><script data-bait="1"></script>';

/** 글마다 태그를 붙인다. 화면이 값으로 갈라 쓰는 칸(종류, 상태, 열쇠, 경로)은 그대로 둔다. */
const KEEP = new Set(['kind', 'page', 'path', 'view', 'type', 'tone', 'status', 'csrf', 'tz', 'now', 'key', 'filter', 'role']);

function poison<T>(value: T, key = ''): T {
  if (typeof value === 'string') {
    if (KEEP.has(key) || /^\d{4}-\d{2}-\d{2}T/.test(value)) return value;
    return (value.startsWith('qz-') || value.length > 24 ? `${value}${BAIT}` : value) as T;
  }
  if (Array.isArray(value)) return value.map((v) => poison(v, key)) as T;
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) out[k] = k === 'consts' ? v : poison(v, k);
    return out as T;
  }
  return value;
}

afterEach(clear);

describe('바깥 글은 글로만 들어간다', () => {
  for (const p of pages('full')) {
    it(p.name, () => {
      const data = poison(p.data) as PageData;
      data.flash = { tone: 'bad', text: `<b data-bait="1">실패</b>${BAIT}` };
      const body = render(data, p.url);
      expect(body.querySelectorAll('[data-bait]').length).toBe(0);
      expect(body.textContent).toContain('<b data-bait="1">실패</b>');
    });
  }
  for (const s of screens()) {
    it(s.name, () => {
      const body = render(poison(s.data) as StandaloneData, s.url);
      expect(body.querySelectorAll('[data-bait]').length).toBe(0);
    });
  }
});
