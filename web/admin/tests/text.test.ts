// 글 보존(결정 0061). 개편 전 화면에 적혀 있던 한국어 글(라벨, 도움말, 안내, 묻는 말, 빈 목록의 한 줄)이
// 새 화면에도 그대로 있는지 본다. 기능 보존 시험은 폼·입력·링크·데이터만 보므로, 글이 줄거나 바뀌는 것은 여기서 잡는다.
//
// 기준: tests/texts.json(지금은 지운 예전 화면 코드에서 뽑아 둔 글. 그때의 기록이라 다시 만들지 않는다). 글은 새 화면의 소스(src)나
// 데몬이 주는 데이터(fixtures: `ui`, `consts`, 안내)에 있으면 된다. 공백은 한 칸으로 줄여 견준다.
// 의도해서 바꾼 글은 tests/text-changes.ts에 까닭과 함께 적는다.
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { ROOT } from './helpers';
import { CHANGED } from './text-changes';

/** 예전 화면 코드 파일 → 그 내용을 옮긴 새 화면 파일(자리 표시가 남아 있으면 아직 옮기는 중이라 건너뛴다). */
const PAGES: Record<string, string[]> = {
  'home.rs': ['pages/Home.svelte'],
  'work.rs': ['pages/Work.svelte'],
  'automation.rs': ['pages/Automation.svelte'],
  'activity.rs': ['pages/Activity.svelte'],
  'lucia.rs': [
    'pages/Memory.svelte',
    'pages/MemoryEntry.svelte',
    'pages/Skills.svelte',
    'pages/SkillDetail.svelte',
    'pages/Character.svelte',
    'pages/CharacterRevision.svelte',
  ],
  'expressions.rs': ['pages/Expressions.svelte'],
  'connections.rs': ['pages/Connections.svelte'],
  'browser.rs': ['pages/Browser.svelte', 'pages/Handover.svelte'],
  'settings.rs': ['pages/SettingsSafety.svelte', 'pages/SettingsNotify.svelte', 'pages/SettingsWork.svelte'],
  'system.rs': ['pages/System.svelte'],
  'standalone.rs': ['standalone/Standalone.svelte'],
  'ui.rs': ['ui/Shell.svelte'],
};

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });
}

const squash = (s: string) => s.replace(/\s+/g, ' ');

// 견줄 곳: 화면 소스 전부(주석 포함: 글이 주석에만 있으면 안 되지만, 그것은 눈으로 본다)와 데몬이 주는 데이터.
const source = squash(
  files(join(ROOT, 'src'))
    .filter((f) => /\.(svelte|ts)$/.test(f) && !f.endsWith('types.gen.ts'))
    .map((f) => readFileSync(f, 'utf8'))
    .join('\n')
    // Svelte 표현식으로 적은 글(`{' · '}`, `{'…'}`)을 마크업의 글과 한데 견주려고 따옴표와 중괄호를 걷어 낸다.
    .replace(/\{(['"`])((?:(?!\1)[^\\{}]|\\.)*)\1\}/g, '$2'),
);
const SERVER = ['derive.rs', 'extra.rs', 'snapshot.rs', 'spa.rs'].map((f) => join(ROOT, '../../crates/luciad/src/admin', f));
const data = squash(
  [
    ...files(join(ROOT, 'fixtures'))
      .filter((f) => f.endsWith('.json') && !f.includes('stress'))
      .map((f) => JSON.stringify(JSON.parse(readFileSync(f, 'utf8'))).replace(/\\"/g, '"')),
    // 시험 데이터에 그 상태가 없어 보이지 않는 글(예: "동의 전")은 데몬이 만드는 곳에서 찾는다.
    ...SERVER.map((f) => readFileSync(f, 'utf8')),
  ].join('\n'),
);

const texts = JSON.parse(readFileSync(join(ROOT, 'tests/texts.json'), 'utf8')) as Record<string, string[]>;

describe('예전 화면의 글이 그대로 있다', () => {
  for (const [old, pages] of Object.entries(PAGES)) {
    it(old, (ctx) => {
      const pending = pages.some((p) => readFileSync(join(ROOT, 'src', p), 'utf8').includes('아직 옮기는 중입니다'));
      if (pending) return ctx.skip();
      const changed = CHANGED[old] ?? {};
      const missing = (texts[old] ?? []).filter((t) => !(t in changed) && !source.includes(t) && !data.includes(t));
      expect(missing, `${old}: 사라지거나 바뀐 글 ${missing.length}개`).toEqual([]);
    });
  }
});
