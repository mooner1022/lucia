// 라우터의 모든 쓰기 경로가 어떤 화면의 폼으로 닿는다(결정 0041의 `every_write_route_has_a_form`을 옮긴 것).
// 데몬에 쓰기 경로를 더하고 화면에 폼을 두지 않으면 여기서 실패한다. 경로는 데몬의 라우터 소스에서 직접 읽는다.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, expect, it } from 'vitest';
import { ROOT, SETS, clear, inventory, pages, render, screens } from './helpers';

/** 동사가 경로에 들어 있는 라우트를 풀어 쓴다(처리기의 `match verb`에서 읽은 값). */
const EXPANDED: Record<string, string[]> = {
  '/goals/{gid}/{verb}': ['review', 'done', 'abandon'],
  '/watches/{wid}/{verb}': ['pause', 'resume', 'remove'],
  '/routines/{rid}/{verb}': ['run', 'pause', 'resume', 'remove'],
  '/skills/{slug}/{verb}': ['approve', 'pin', 'unpin', 'archive', 'restore'],
};

/** 폼이 아닌 길로 닿는 쓰기 경로. */
const NOT_A_FORM: Record<string, string> = {};

function postRoutes(): string[] {
  const source = readFileSync(join(ROOT, '../../crates/luciad/src/admin/mod.rs'), 'utf8');
  const start = source.indexOf('pub fn router(');
  const end = source.indexOf('/// 바인드 주소마다', start);
  expect(start).toBeGreaterThan(0);
  expect(end).toBeGreaterThan(start);
  const router = source.slice(start, end);
  const starts = [...router.matchAll(/\.route\(\s*"([^"]+)"/g)].map((m) => ({ at: m.index, pattern: m[1] }));
  const out: string[] = [];
  starts.forEach((route, i) => {
    let stop = i + 1 < starts.length ? starts[i + 1].at : router.length;
    const layer = router.slice(route.at, stop).indexOf('.layer(');
    if (layer >= 0) stop = route.at + layer;
    if (router.slice(route.at, stop).includes('post(')) out.push(route.pattern);
  });
  return out;
}

afterEach(clear);

it('모든 쓰기 경로에 폼이 있다', () => {
  const routes = postRoutes();
  expect(routes.length).toBeGreaterThan(50);

  const forms = new Set<string>();
  const collect = (body: HTMLElement) => {
    for (const line of inventory(body)) {
      if (line.startsWith('form post ')) forms.add(line.slice('form post '.length).split(' ')[0]);
    }
  };
  for (const set of SETS) for (const p of pages(set)) collect(render(p.data, p.url));
  for (const s of screens()) collect(render(s.data, s.url));

  const unreachable: string[] = [];
  for (const pattern of routes) {
    if (pattern in NOT_A_FORM) continue;
    const concrete = EXPANDED[pattern]?.map((verb) => pattern.replace('{verb}', verb)) ?? [pattern];
    for (const path of concrete) {
      const escaped = path.replace(/[.*+?^$()|[\]\\]/g, '\\$&').replace(/\{[a-z]+\}/g, '[^/?]+');
      const re = new RegExp(`^${escaped}(\\?.*)?$`);
      if (![...forms].some((action) => re.test(action))) unreachable.push(path);
    }
  }
  expect(unreachable, `화면에서 닿지 못하는 쓰기 경로 ${unreachable.length}개`).toEqual([]);
});
