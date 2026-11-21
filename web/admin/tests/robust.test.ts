// 빈 값에 견디기(결정 0061). 시험 데이터는 칸이 대부분 채워져 있다. 실제 데이터에서는 비어 있을 수 있는 칸
// (데몬의 `Option`)이 null로 오고 목록이 빈다. 그때 화면이 통째로 죽지 않는지 본다: 오류 경계가 던진 것을 삼키므로
// "이 화면을 그리지 못했습니다" 카드가 보이는지로 판단한다.
//
// 평소에는 한꺼번에 비운다(화면마다 두 번). `SWEEP=1`이면 칸 이름 하나씩 비워 가며 본다(느리다. 화면을 크게 고친 뒤에).
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import type { PageData } from '../src/lib/types';
import { ROOT, clear, pages, render } from './helpers';

function rustFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? rustFiles(join(dir, e.name)) : e.name.endsWith('.rs') ? [join(dir, e.name)] : [],
  );
}

/** 데몬의 어느 타입에서든 `Option`으로 적힌 칸 이름(넉넉하게 잡는다). */
const optional = new Set<string>();
for (const crate of ['lucia-store', 'lucia-channel', 'luciad']) {
  for (const file of rustFiles(join(ROOT, '../../crates', crate, 'src'))) {
    for (const m of readFileSync(file, 'utf8').matchAll(/^\s*(?:pub(?:\([a-z: ]+\))? )?([a-z_][a-z0-9_]*): Option</gm)) {
      optional.add(m[1]);
    }
  }
}

/**
 * 이름은 같지만 **이 자리에서는** 비지 않는 칸. 데몬의 타입에서 그 자리가 `Option`이 아닌 것을 확인하고 적는다.
 * (다른 타입의 같은 이름 칸이 Option이라 이름만으로는 걸린다.)
 */
const NEVER_NULL: Record<string, string> = {
  // 스냅숏의 묶음(늘 있는 구조체)과 화면 덧붙임의 칸.
  app: 'Snapshot.app: AppView',
  backup: 'Snapshot.backup: BackupView',
  discord: 'Snapshot.discord: DiscordView',
  images: 'Snapshot.images: ImagesView',
  profile: 'Snapshot.profile: ProfileView(Option인 것은 도구의 profile: Option<ProfileAccess>다, 결정 0064)',
  'discord.state': 'DiscordView.state: DiscordState(연결 상태는 늘 있다. Option인 것은 SourceView.state다)',
  'images.daily_cap': 'ImagesView.daily_cap: f64(Option인 것은 Snapshot.daily_cap이다)',
  goal: 'GoalView.goal: Goal',
  routine: 'RoutineView.routine: Routine',
  file: 'KeptFile.file: FileRecord, Expression.file: ExpressionFile',
  model: 'extra.model(캐릭터 화면의 주 모델 묶음, extra::character가 늘 만든다)',
  query: 'extra.query(기억 찾기의 말, 없으면 빈 글)',
  revision: 'extra.revision(캐릭터 개정), Skill.revision: i64',
  // 저장소 줄의 글 칸: 모두 String이다.
  body: 'MemoryEntry·MemoryRevision·SkillRevision.body: String',
  summary: 'Activity·Approval·Signal·MemoryEntry·MemoryRevision.summary: String',
  // 줄을 가르는 값: 목록의 열쇠로 쓴다.
  id: '저장소 줄의 id는 늘 있다',
  key: '이름표 목록의 key는 늘 있다',
};

/** 건드리지 않는 곳: 화면이 값으로 갈라 쓰는 틀의 칸. */
const KEEP = new Set(['kind', 'page', 'path', 'title', 'csrf', 'consts', 'notices', 'overall', 'pending', 'ui', 'view', 'tz', 'now']);

/** `change`에는 칸 이름, 값, 그 칸을 가진 묶음의 이름(부모)이 간다. */
function mutate(
  value: unknown,
  change: (key: string, v: unknown, parent: string) => unknown | undefined,
  key = '',
  parent = '',
): unknown {
  if (Array.isArray(value)) {
    const swapped = change(key, value, parent);
    if (swapped !== undefined) return swapped;
    return value.map((v) => mutate(v, change, key, parent));
  }
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) {
      if (KEEP.has(k)) {
        out[k] = v;
        continue;
      }
      const swapped = Array.isArray(v) ? undefined : change(k, v, key);
      out[k] = swapped !== undefined ? swapped : mutate(v, change, k, key);
    }
    return out;
  }
  return value;
}

const NULL = Symbol('null');
const never = (key: string, parent: string) => key in NEVER_NULL || `${parent}.${key}` in NEVER_NULL;
const toNull = (names: Set<string>) => (key: string, v: unknown, parent: string) =>
  names.has(key) && !never(key, parent) && v !== null && !Array.isArray(v) ? NULL : undefined;
const unwrap = (v: unknown): unknown =>
  v === NULL ? null : Array.isArray(v) ? v.map(unwrap) : v && typeof v === 'object' ? Object.fromEntries(Object.entries(v).map(([k, x]) => [k, unwrap(x)])) : v;

function broken(data: PageData, url: string): string | null {
  const body = render(data, url);
  return body.textContent?.includes('이 화면을 그리지 못했습니다') ? (body.querySelector('[role="alert"]')?.textContent ?? '').slice(0, 200) : null;
}

afterEach(clear);

describe('빈 값이 와도 화면이 죽지 않는다', () => {
  for (const p of pages('full')) {
    it(`${p.name}: 비어 있을 수 있는 칸이 모두 null`, () => {
      const data = unwrap(mutate(p.data, toNull(optional))) as PageData;
      expect(broken(data, p.url)).toBeNull();
    });
    it(`${p.name}: 목록이 모두 빔`, () => {
      const data = mutate(p.data, (_key, v) => (Array.isArray(v) ? [] : undefined)) as PageData;
      expect(broken(data, p.url)).toBeNull();
    });
  }
});

describe.runIf(process.env.SWEEP === '1')('칸 이름 하나씩 비워 보기', () => {
  for (const p of pages('full')) {
    it(p.name, () => {
      const present = new Set<string>();
      mutate(p.data, (key) => {
        if (optional.has(key)) present.add(key);
        return undefined;
      });
      const failed: string[] = [];
      for (const name of present) {
        const data = unwrap(mutate(p.data, toNull(new Set([name])))) as PageData;
        const why = broken(data, p.url);
        if (why) failed.push(`${name}: ${why}`);
        clear();
      }
      expect(failed).toEqual([]);
    });
  }
});
