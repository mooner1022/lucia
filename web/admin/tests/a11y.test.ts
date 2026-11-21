// 접근성 시험(결정 0041·0061). 예전 데몬의 `view/a11y.rs`(렌더된 HTML을 훑던 시험)를 화면 쪽으로 옮긴 것이다.
// 템플릿이 사라져도 같은 기준이 남도록, 여기서는 **새 화면의 DOM**과 **새 스타일 파일**에 같은 기준을 건다.
//
// 1. 마크업: 모든 화면(상태 묶음 full·alt·fresh의 모든 화면, 하위 화면, 단독 화면)을 그려 규칙마다 위반을 모은다.
//    위반은 "화면(묶음:이름) · 규칙 · 요소 발췌"로 알린다. 같은 위반이 여러 화면에 있으면(틀의 문제) 한 줄로 묶는다.
//    자리 표시 글('아직 옮기는 중입니다')이 보이는 화면은 건너뛴다(parity.test.ts와 같다). 옮기고 나면 바로 걸린다.
// 2. 색: tokens.css를 읽어 밝은·어두운 판의 토큰 짝이 WCAG AA(글 4.5:1, 경계·초점·그림 3:1)를 지키는지 계산한다.
// 3. 스타일: base.css·tokens.css·부품(.svelte의 <style>, field.css)이 접근성 장치를 지니는지 본다.
//
// 한 규칙만 돌리기: npx vitest run tests/a11y.test.ts -t "heading-order"
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { NAV, isCurrent } from '../src/lib/nav';
import type { PageData, StandaloneData } from '../src/lib/types';
import { ROOT, clear, pages, render, screens } from './helpers';

/** 아직 옮기지 않은 화면의 자리 표시 글. 이 글이 보이는 화면은 건너뛴다. */
const PLACEHOLDER = '아직 옮기는 중입니다';
/** 마크업을 훑는 상태 묶음(모든 목록이 찬 것, 분기를 바꾼 것, 처음 쓰는 것). */
const MARKUP_SETS = ['full', 'alt', 'fresh'] as const;

// ───────────────────────── 1. 마크업 ─────────────────────────

const norm = (s: string) => s.replace(/\s+/g, ' ').trim();

/** 실패 메시지에 넣는 요소 발췌(태그 모양을 알아볼 만큼만). */
function shot(el: Element | null, max = 150): string {
  if (!el) return '(없음)';
  const html = norm(el.outerHTML);
  return html.length > max ? `${html.slice(0, max)}…` : html;
}

/**
 * 보조 기술이 읽는 글. svg, aria-hidden, hidden 부분은 뺀다. 그림은 대체 글을 읽는다.
 * `visible`이면 화면에 보이는 글만(.sr-only도 뺀다). 입력 칸 안의 글(고르기 항목, 여러 줄 입력)은 라벨의 글이 아니다.
 */
function readable(el: Element, visible = false): string {
  let out = '';
  const walk = (node: Node) => {
    if (node.nodeType === 3) {
      out += node.textContent ?? '';
      return;
    }
    if (node.nodeType !== 1) return;
    const e = node as Element;
    const tag = e.tagName.toLowerCase();
    if (['svg', 'script', 'style', 'select', 'textarea', 'option'].includes(tag)) return;
    if (e.getAttribute('aria-hidden') === 'true' || e.hasAttribute('hidden')) return;
    if (visible && e.classList.contains('sr-only')) return;
    if (tag === 'img' || (tag === 'input' && e.getAttribute('type') === 'image')) {
      if (!visible) out += ` ${e.getAttribute('alt') ?? ''} `;
      return;
    }
    for (const child of e.childNodes) walk(child);
  };
  for (const child of el.childNodes) walk(child);
  return norm(out);
}

/** 이름: aria-label, aria-labelledby, 안의 글(그림 대체 글 포함) 차례. */
function nameOf(el: Element, doc: Document): string {
  const label = norm(el.getAttribute('aria-label') ?? '');
  if (label) return label;
  const by = (el.getAttribute('aria-labelledby') ?? '').split(/\s+/).filter(Boolean);
  const named = norm(by.map((id) => doc.getElementById(id)?.textContent ?? '').join(' '));
  if (named) return named;
  return readable(el);
}

interface Ctx {
  root: HTMLElement;
  doc: Document;
  data: PageData | StandaloneData;
}

interface MarkupRule {
  /** 시험 이름이자 실패 메시지의 규칙 이름. */
  id: string;
  /** 예전 시험의 이름(옮긴 곳 표시용). */
  from: string;
  run: (c: Ctx) => string[];
}

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([type="hidden"]):not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

const isPage = (d: PageData | StandaloneData): d is PageData => d.kind === 'page';

/** 메뉴 칸 목록(키 → 경로). 사이드바·펼침·아래 막대를 알아보는 데 쓴다. */
const NAV_ITEMS = NAV.flatMap((g) => g.pages);
const NAV_PATHS = new Set(NAV_ITEMS.map((p) => p.path));

const VALID_CURRENT = new Set(['page', 'step', 'location', 'date', 'time', 'true', 'false']);

const MARKUP_RULES: MarkupRule[] = [
  {
    id: 'h1-one',
    from: 'every_page_has_the_basic_landmarks (h1)',
    run: ({ root }) => {
      const h1 = [...root.querySelectorAll('h1')];
      if (h1.length === 1) return [];
      return [`h1이 ${h1.length}개(하나여야 한다)${h1.map((h) => ` · ${shot(h, 80)}`).join('')}`];
    },
  },
  {
    id: 'main-one',
    from: 'every_page_has_the_basic_landmarks (main, id="main")',
    run: ({ root }) => {
      const main = [...root.querySelectorAll('main, [role="main"]')];
      if (main.length !== 1) return [`main이 ${main.length}개(하나여야 한다)${main.map((m) => ` · ${shot(m, 80)}`).join('')}`];
      return main[0].id === 'main' ? [] : [`main에 id="main"이 없다(건너뛰기 링크의 대상): ${shot(main[0], 100)}`];
    },
  },
  {
    id: 'skip-link',
    from: 'every_page_has_the_basic_landmarks (건너뛰기 링크)',
    run: ({ root, data }) => {
      // 사이드바가 있는 로그인한 화면만. 단독 화면은 메뉴가 없어 건너뛸 것이 없다.
      if (!isPage(data)) return [];
      const skip = [...root.querySelectorAll('a[href="#main"]')];
      if (skip.length === 0) return ['로그인한 화면에 본문 건너뛰기 링크(a[href="#main"])가 없다'];
      const first = root.querySelector(FOCUSABLE);
      return first === skip[0] ? [] : [`건너뛰기 링크가 첫 번째로 초점이 가는 요소가 아니다. 먼저 오는 것: ${shot(first, 100)}`];
    },
  },
  {
    id: 'nav-name',
    from: 'every_page_has_the_basic_landmarks (nav 이름)',
    run: ({ root, doc }) => {
      const hits: string[] = [];
      const seen = new Map<string, Element>();
      for (const nav of root.querySelectorAll('nav')) {
        const name = nameFromAttrs(nav, doc);
        if (!name) {
          hits.push(`nav에 이름(aria-label)이 없다: ${shot(nav, 100)}`);
          continue;
        }
        const other = seen.get(name);
        if (other) hits.push(`nav 이름이 겹친다("${name}"): ${shot(nav, 100)}`);
        else seen.set(name, nav);
      }
      return hits;
    },
  },
  {
    id: 'aria-current',
    from: '(새로 둠: 예전 템플릿은 메뉴가 하나라 따로 안 봤다)',
    run: ({ root, data }) => {
      const hits: string[] = [];
      for (const nav of root.querySelectorAll('nav')) {
        const current = [...nav.querySelectorAll('[aria-current]')];
        for (const el of current) {
          const v = el.getAttribute('aria-current') ?? '';
          if (!VALID_CURRENT.has(v)) hits.push(`aria-current 값이 틀렸다("${v}"): ${shot(el, 100)}`);
        }
        const marked = current.filter((el) => el.getAttribute('aria-current') === 'page');
        if (marked.length > 1) hits.push(`한 메뉴에 aria-current="page"가 ${marked.length}개: ${shot(nav, 90)}`);
        if (!isPage(data)) continue;
        // 사이드바·펼침 메뉴·아래 막대처럼 메뉴 칸으로 이루어진 nav: 지금 화면의 칸이 켜져 있어야 한다.
        const links = [...nav.querySelectorAll('a[href]')].map((a) => a.getAttribute('href') ?? '');
        const isSite = links.length >= 2 && links.every((h) => NAV_PATHS.has(h));
        if (!isSite) continue;
        const item = NAV_ITEMS.find((p) => isCurrent(p, data.page));
        if (!item) {
          hits.push(`화면 "${data.page}"에 해당하는 메뉴 칸이 없다`);
          continue;
        }
        // 아래 막대는 자주 쓰는 칸만 두므로, 그 칸이 없으면 켜진 것이 없는 것이 맞다.
        const mustHave = links.includes(item.path);
        const hrefs = marked.map((el) => el.getAttribute('href'));
        if (mustHave && !(hrefs.length === 1 && hrefs[0] === item.path)) {
          hits.push(`현재 화면(${item.path})의 칸만 aria-current="page"여야 한다. 지금: [${hrefs.join(', ')}] — ${shot(nav, 90)}`);
        }
        if (!mustHave && marked.length > 0) hits.push(`이 메뉴에 없는 화면인데 켜진 칸이 있다: [${hrefs.join(', ')}]`);
      }
      return hits;
    },
  },
  {
    id: 'heading-order',
    from: 'headings_descend_one_level_at_a_time',
    run: ({ root }) => {
      const hits: string[] = [];
      const level = (h: Element) => Number(h.tagName[1]);
      const walk = (heads: Element[], start: number, where: string) => {
        let prev = start;
        for (const h of heads) {
          const l = level(h);
          if (l > prev + 1) hits.push(`${where}h${prev} 다음에 h${l}(한 단계씩만 내려간다): ${shot(h, 100)}`);
          prev = l;
        }
      };
      const all = [...root.querySelectorAll('h1, h2, h3, h4, h5, h6')];
      for (const h of all) if (!readable(h)) hits.push(`글 없는 제목: ${shot(h, 100)}`);
      // 묻는 창·펼침 메뉴(dialog)는 본문 제목 차례에 끼지 않고, 안에서 h2부터 다시 시작해도 된다.
      walk(
        all.filter((h) => !h.closest('dialog')),
        0,
        '',
      );
      for (const d of root.querySelectorAll('dialog')) {
        walk([...d.querySelectorAll('h1, h2, h3, h4, h5, h6')], 1, `dialog(${shot(d, 60)}) 안: `);
      }
      return hits;
    },
  },
  {
    id: 'id-unique',
    from: 'ids_are_unique_and_references_resolve (id 유일)',
    run: ({ root }) => {
      const byId = new Map<string, Element[]>();
      for (const el of root.querySelectorAll('[id]')) byId.set(el.id, [...(byId.get(el.id) ?? []), el]);
      return [...byId].filter(([, list]) => list.length > 1).map(([id, list]) => `id가 겹친다("${id}" ${list.length}개): ${shot(list[1], 100)}`);
    },
  },
  {
    id: 'refs-resolve',
    from: 'ids_are_unique_and_references_resolve (for, aria-*, #링크)',
    run: ({ root, doc }) => {
      const hits: string[] = [];
      for (const label of root.querySelectorAll('label[for]')) {
        const target = doc.getElementById(label.getAttribute('for') ?? '');
        if (!target) hits.push(`label for가 가리키는 id 없음: ${shot(label, 100)}`);
        else if (!target.matches('input:not([type="hidden"]), select, textarea, button, meter, output, progress')) {
          hits.push(`label for가 라벨을 달 수 없는 요소를 가리킨다: ${shot(label, 100)} → ${shot(target, 80)}`);
        }
      }
      for (const attr of ['aria-labelledby', 'aria-describedby', 'aria-controls', 'aria-owns', 'aria-activedescendant', 'aria-errormessage', 'aria-details']) {
        for (const el of root.querySelectorAll(`[${attr}]`)) {
          for (const id of (el.getAttribute(attr) ?? '').split(/\s+/).filter(Boolean)) {
            if (!doc.getElementById(id)) hits.push(`${attr}가 가리키는 id 없음("${id}"): ${shot(el, 100)}`);
          }
        }
      }
      // 링크 `#구역`은 그 구역이 있는 화면에서만 쓴다.
      for (const a of root.querySelectorAll('a[href^="#"]')) {
        const id = decodeURIComponent((a.getAttribute('href') ?? '').slice(1));
        if (!id) hits.push(`a href="#"는 갈 곳이 없다: ${shot(a, 100)}`);
        else if (!doc.getElementById(id)) hits.push(`화면 안 링크의 대상 없음(#${id}): ${shot(a, 100)}`);
      }
      return hits;
    },
  },
  {
    id: 'control-label',
    from: 'every_visible_control_has_a_label',
    run: ({ root, doc }) => {
      const hits: string[] = [];
      for (const el of root.querySelectorAll<HTMLInputElement>('input:not([type="hidden"]), select, textarea')) {
        const type = (el.getAttribute('type') ?? '').toLowerCase();
        if (type === 'image') continue; // 이미지 입력은 아래 img-alt가 본다(대체 글이 이름이다).
        if (['submit', 'button', 'reset'].includes(type)) {
          if (!norm(el.getAttribute('aria-label') ?? '') && !norm(el.getAttribute('value') ?? '')) hits.push(`이름 없는 입력 단추: ${shot(el, 110)}`);
          continue;
        }
        const labels = [...(el.labels ?? [])].map((l) => readable(l)).join(' ').trim();
        if (!nameFromAttrs(el, doc) && !labels) hits.push(`라벨 없는 입력: ${shot(el, 120)}`);
      }
      return hits;
    },
  },
  {
    id: 'interactive-text',
    from: 'buttons_links_and_summaries_have_text',
    run: ({ root, doc }) => {
      const hits: string[] = [];
      for (const [kind, sel] of [
        ['단추', 'button'],
        ['링크', 'a[href]'],
        ['summary', 'summary'],
      ] as const) {
        for (const el of root.querySelectorAll(sel)) {
          if (!nameOf(el, doc)) hits.push(`글 없는 ${kind}(이름: 글, aria-label, 그림 alt): ${shot(el, 130)}`);
        }
      }
      return hits;
    },
  },
  {
    id: 'widget-name',
    from: '(새로 둠: Progress, 묻는 창, 펼침 메뉴의 이름)',
    run: ({ root, doc }) => {
      const hits: string[] = [];
      for (const el of root.querySelectorAll('dialog')) {
        if (!nameFromAttrs(el, doc)) hits.push(`이름 없는 dialog(aria-label이나 aria-labelledby): ${shot(el, 100)}`);
      }
      for (const el of root.querySelectorAll<HTMLElement>('progress, meter, [role="progressbar"]')) {
        const labels = [...((el as HTMLProgressElement).labels ?? [])].map((l) => readable(l)).join(' ').trim();
        if (!nameFromAttrs(el, doc) && !labels) hits.push(`이름 없는 진행 표시: ${shot(el, 100)}`);
      }
      return hits;
    },
  },
  {
    id: 'details-summary',
    from: 'buttons_links_and_summaries_have_text (details마다 summary 하나)',
    run: ({ root }) => {
      const hits: string[] = [];
      for (const d of root.querySelectorAll('details')) {
        const own = [...d.children].filter((c) => c.tagName === 'SUMMARY');
        if (own.length !== 1) hits.push(`details에 summary가 ${own.length}개(하나여야 한다): ${shot(d, 100)}`);
        else if (d.firstElementChild !== own[0]) hits.push(`summary가 details의 첫 요소가 아니다: ${shot(d, 100)}`);
      }
      return hits;
    },
  },
  {
    id: 'img-alt',
    from: 'every_visible_control_has_a_label (이미지 입력 alt) + (img alt)',
    run: ({ root }) => {
      const hits: string[] = [];
      for (const img of root.querySelectorAll('img')) {
        if (!img.hasAttribute('alt')) hits.push(`alt 없는 img(장식이면 alt=""): ${shot(img, 110)}`);
      }
      for (const input of root.querySelectorAll('input[type="image"]')) {
        if (!norm(input.getAttribute('alt') ?? '')) hits.push(`대체 글 없는 이미지 입력: ${shot(input, 110)}`);
      }
      return hits;
    },
  },
  {
    id: 'table-scope',
    from: 'tables_have_header_cells_with_scope',
    run: ({ root }) => {
      const hits: string[] = [];
      for (const table of root.querySelectorAll('table')) {
        const ths = [...table.querySelectorAll('th')];
        if (ths.length === 0) hits.push(`머리 칸(th)이 없는 표: ${shot(table, 100)}`);
        for (const th of ths) {
          if (!['col', 'row', 'colgroup', 'rowgroup'].includes(th.getAttribute('scope') ?? '')) hits.push(`scope 없는 th: ${shot(th, 100)}`);
        }
      }
      return hits;
    },
  },
  {
    id: 'icon-hidden',
    from: 'status_is_never_color_only (장식 아이콘 숨김)',
    run: ({ root, doc }) => {
      const hits: string[] = [];
      for (const svg of root.querySelectorAll('svg')) {
        const labelled = svg.getAttribute('role') === 'img' && nameFromAttrs(svg, doc);
        if (svg.getAttribute('aria-hidden') !== 'true' && !labelled) hits.push(`보조 기술에 보이는 svg(aria-hidden="true"여야 한다): ${shot(svg, 100)}`);
      }
      return hits;
    },
  },
  {
    id: 'dot-badge-text',
    from: 'status_is_never_color_only',
    run: ({ root }) => {
      const hits: string[] = [];
      // 점은 장식이다: 곁의 글이 뜻을 말하고, 점 자체는 보조 기술에서 숨긴다. 글 없이 점만 있으면 색만으로 알리는 것이다.
      for (const dot of root.querySelectorAll('.dot, .pip')) {
        if (dot.getAttribute('aria-hidden') !== 'true') hits.push(`점이 보조 기술에 보인다(aria-hidden="true"여야 한다): ${shot(dot, 100)}`);
        const parent = dot.parentElement;
        if (!parent || !readable(parent, true)) hits.push(`점 곁에 보이는 글이 없다(색만으로 알린다): ${shot(parent ?? dot, 120)}`);
      }
      for (const badge of root.querySelectorAll('.badge')) {
        if (!readable(badge, true)) hits.push(`글 없는 배지(아이콘만으로 알린다): ${shot(badge, 120)}`);
      }
      return hits;
    },
  },
  {
    id: 'aria-hidden-focus',
    from: '(새로 둠: 숨긴 것 안에 초점이 가는 것이 없다)',
    run: ({ root }) => {
      const hits: string[] = [];
      for (const el of root.querySelectorAll('[aria-hidden="true"]')) {
        if (el.matches(FOCUSABLE) || el.querySelector(FOCUSABLE)) hits.push(`aria-hidden 안에 초점이 가는 것이 있다: ${shot(el, 120)}`);
      }
      return hits;
    },
  },
  {
    id: 'flash-role',
    from: '(새로 둠: 처리 결과 안내는 role="status"나 "alert")',
    run: ({ root, data }) => {
      // 화면이 서버의 처리 결과 안내를 달고 왔다면, 그 안내가 보조 기술에 알려져야 한다.
      if (!isPage(data) || !data.flash) return [];
      const banner = [...root.querySelectorAll('main p')].find((p) => p.textContent === data.flash?.text)?.parentElement;
      if (!banner) return [`처리 결과 안내("${data.flash.text}")를 화면에서 찾지 못했다`];
      const role = banner.getAttribute('role');
      return role === 'status' || role === 'alert' ? [] : [`처리 결과 안내에 role="status"나 "alert"가 없다: ${shot(banner, 120)}`];
    },
  },
  {
    id: 'document-title',
    from: 'every_page_has_the_basic_landmarks (title)',
    run: ({ doc }) => (norm(doc.title) ? [] : ['화면을 그린 뒤 문서 제목(document.title)이 비어 있다']),
  },
];

/** aria-label, aria-labelledby만 본 이름(입력의 라벨 글과 따로). */
function nameFromAttrs(el: Element, doc: Document): string {
  const label = norm(el.getAttribute('aria-label') ?? '');
  if (label) return label;
  const by = (el.getAttribute('aria-labelledby') ?? '').split(/\s+/).filter(Boolean);
  return norm(by.map((id) => doc.getElementById(id)?.textContent ?? '').join(' '));
}

interface Target {
  label: string;
  data: PageData | StandaloneData;
  url: string;
}

const TARGETS: Target[] = [
  ...MARKUP_SETS.flatMap((set) => pages(set).map((p) => ({ label: `${set}:${p.name}`, data: p.data as PageData | StandaloneData, url: p.url }))),
  ...screens().map((s) => ({ label: `단독:${s.name}`, data: s.data as PageData | StandaloneData, url: s.url })),
];

/** 화면 하나를 한 번만 그려 모든 규칙을 돌린다(규칙마다 다시 그리지 않는다). 자리 표시 화면은 null. */
const audited = new Map<string, Map<string, string[]> | null>();
function audit(target: Target): Map<string, string[]> | null {
  if (audited.has(target.label)) return audited.get(target.label)!;
  document.title = '';
  const root = render(target.data, target.url);
  let result: Map<string, string[]> | null = null;
  if (!root.textContent?.includes(PLACEHOLDER)) {
    result = new Map();
    const ctx: Ctx = { root, doc: root.ownerDocument, data: target.data };
    for (const rule of MARKUP_RULES) result.set(rule.id, rule.run(ctx));
  }
  audited.set(target.label, result);
  return result;
}

/** 같은 위반은 한 줄로 묶고, 어느 화면들인지 붙인다. */
function failMessage(title: string, groups: Map<string, string[]>): string {
  const lines = [...groups].slice(0, 40).map(([hit, labels]) => {
    const shown = labels.slice(0, 4).join(', ');
    const more = labels.length > 4 ? ` 외 ${labels.length - 4}쪽` : '';
    return `  - ${hit}\n      화면: ${shown}${more} (${labels.length}쪽)`;
  });
  const rest = groups.size > 40 ? `\n  … 그 밖에 ${groups.size - 40}종` : '';
  return `${title}: 위반 ${groups.size}종\n${lines.join('\n')}${rest}`;
}

afterAll(clear);

describe('마크업: 모든 화면에 같은 기준', () => {
  for (const rule of MARKUP_RULES) {
    it(`${rule.id} (예전: ${rule.from})`, (ctx) => {
      const groups = new Map<string, string[]>();
      let checked = 0;
      for (const target of TARGETS) {
        const result = audit(target);
        if (!result) continue;
        checked++;
        for (const hit of result.get(rule.id) ?? []) groups.set(hit, [...(groups.get(hit) ?? []), target.label]);
      }
      if (checked === 0) return ctx.skip();
      if (groups.size > 0) throw new Error(failMessage(`[${rule.id}] 규칙 위반`, groups));
    });
  }

  it('검사한 화면이 있다(자리 표시 화면만 있어 아무것도 안 본 채로 통과하지 않는다)', (ctx) => {
    const done = TARGETS.filter((t) => audit(t));
    if (done.length === 0) return ctx.skip();
    expect(done.length).toBeGreaterThan(0);
  });
});

describe('처리 결과 안내', () => {
  // 시험 데이터의 안내는 한 가지 결뿐이라, 결마다 안내를 달아 그려 본다(escape.test.ts와 같은 방법).
  for (const tone of ['ok', 'info', 'warn', 'bad'] as const) {
    it(`${tone} 안내는 role="status"나 "alert"다`, () => {
      const target = TARGETS.find((t) => t.label === 'full:/');
      if (!target) throw new Error('시험 데이터에 full:/ 화면이 없다');
      const data = structuredClone(target.data) as PageData;
      data.flash = { tone, text: `안내-${tone}` };
      const root = render(data, target.url);
      const banner = [...root.querySelectorAll('main p')].find((p) => p.textContent === `안내-${tone}`)?.parentElement;
      expect(banner, `[flash-role] full:/ · ${tone} 안내가 main 안에 없다`).toBeTruthy();
      const role = banner!.getAttribute('role');
      expect(['status', 'alert'], `[flash-role] full:/ · ${tone} 안내에 role이 없다: ${shot(banner)}`).toContain(role);
    });
  }
});

describe('문서 틀(index.html)', () => {
  // 예전에는 템플릿이 <html lang>, <title>, viewport를 그렸다. 지금은 정적 index.html이 한다.
  const html = readFileSync(join(ROOT, 'index.html'), 'utf8');
  it('lang="ko", viewport, 첫 제목', () => {
    expect(html, 'index.html: <html lang="ko">').toMatch(/<html[^>]*\slang="ko"/);
    expect(html, 'index.html: <title>').toMatch(/<title>[^<]+<\/title>/);
    const viewport = /<meta[^>]*name="viewport"[^>]*content="([^"]*)"/.exec(html)?.[1];
    expect(viewport, 'index.html: viewport 메타').toBeTruthy();
    expect(viewport, 'viewport가 width=device-width').toContain('width=device-width');
  });
  it('확대를 막지 않는다(WCAG 1.4.4)', () => {
    const viewport = /<meta[^>]*name="viewport"[^>]*content="([^"]*)"/.exec(html)?.[1] ?? '';
    expect(viewport, `viewport가 확대를 막는다: ${viewport}`).not.toMatch(/user-scalable\s*=\s*(no|0)/);
    const max = /maximum-scale\s*=\s*([0-9.]+)/.exec(viewport)?.[1];
    if (max) expect(Number(max), `viewport maximum-scale=${max}`).toBeGreaterThanOrEqual(2);
  });
});

// ───────────────────────── 2. 색 ─────────────────────────

const stripComments = (css: string) => css.replace(/\/\*[\s\S]*?\*\//g, '');
const read = (path: string) => readFileSync(join(ROOT, path), 'utf8');

/** 규칙 하나: 둘러싼 at-규칙(@media …)과 선택자, 선언. */
interface CssRule {
  at: string;
  selector: string;
  decl: Map<string, string>;
}

function declarations(body: string): Map<string, string> {
  const out = new Map<string, string>();
  for (const part of body.split(';')) {
    const i = part.indexOf(':');
    if (i < 0) continue;
    const name = part.slice(0, i).trim();
    if (!/^(--)?[a-zA-Z][a-zA-Z0-9-]*$/.test(name)) continue;
    out.set(name, norm(part.slice(i + 1)));
  }
  return out;
}

/** 중첩된 @media·@container를 풀어 규칙 목록으로(@keyframes·@font-face는 건너뛴다). */
function parseCss(source: string, at = ''): CssRule[] {
  const css = stripComments(source);
  const out: CssRule[] = [];
  let i = 0;
  while (i < css.length) {
    const open = css.indexOf('{', i);
    if (open < 0) break;
    let depth = 1;
    let j = open + 1;
    while (j < css.length && depth > 0) {
      if (css[j] === '{') depth++;
      else if (css[j] === '}') depth--;
      j++;
    }
    const prelude = norm(css.slice(i, open).split(';').pop() ?? '');
    const body = css.slice(open + 1, j - 1);
    if (prelude.startsWith('@')) {
      if (!/^@(keyframes|font-face)/.test(prelude)) out.push(...parseCss(body, at ? `${at} and ${prelude}` : prelude));
    } else {
      out.push({ at, selector: prelude, decl: declarations(body) });
    }
    i = j;
  }
  return out;
}

/** `marker`로 시작하는 규칙의 선언(중괄호가 맞는 곳까지, 따옴표는 작은따옴표로 맞춘다). */
function blockOf(css: string, marker: RegExp): Map<string, string> {
  const text = stripComments(css).replace(/"/g, "'");
  const start = text.search(marker);
  if (start < 0) throw new Error(`블록 없음: ${marker}`);
  const open = text.indexOf('{', start);
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    if (text[i] === '{') depth++;
    else if (text[i] === '}' && --depth === 0) return declarations(text.slice(open + 1, i));
  }
  throw new Error(`닫히지 않은 블록: ${marker}`);
}

const TOKENS_CSS = read('src/ui/tokens.css');
const LIGHT = blockOf(TOKENS_CSS, /^:root\s*\{/m);
const DARK = blockOf(TOKENS_CSS, /:root\[data-theme='dark'\]\s*\{/);
// 기기 설정을 따르는 어두운 판(@media)과 사용자가 고른 어두운 판이 같아야 한다.
const DARK_MEDIA = blockOf(TOKENS_CSS, /:root:not\(\[data-theme='light'\]\)\s*\{/);

const HEX = /^#[0-9a-f]{6}$/i;
/** 6자리 색인 토큰만(이름은 `--` 없이). */
const hexTokens = (m: Map<string, string>) =>
  new Map([...m].filter(([, v]) => HEX.test(v)).map(([k, v]): [string, string] => [k.replace(/^--/, ''), v.toLowerCase()]));

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function ratio(a: string, b: string): number {
  const [la, lb] = [luminance(a), luminance(b)];
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** 색을 검정(또는 흰색) 쪽으로 t만큼 섞는다. */
function mix(hex: string, toward: '#000000' | '#ffffff', t: number): string {
  const channel = (i: number) => {
    const from = parseInt(hex.slice(i, i + 2), 16);
    const to = parseInt(toward.slice(i, i + 2), 16);
    return Math.round(from + (to - from) * t)
      .toString(16)
      .padStart(2, '0');
  };
  return `#${channel(1)}${channel(3)}${channel(5)}`;
}

/** 모든 짝(prefs)을 통과하도록 `color`를 `bg`와 멀어지는 쪽으로 조금씩 옮겨 본 가장 가까운 값. 안 되면 null. */
function nudge(color: string, toward: '#000000' | '#ffffff', checks: { other: string; min: number }[]): string | null {
  for (let t = 0; t <= 1; t += 0.01) {
    const candidate = mix(color, toward, t);
    // 여유 0.1: 딱 기준선에 걸친 값을 권하지 않는다.
    if (checks.every((c) => ratio(candidate, c.other) >= c.min + 0.1)) return candidate;
  }
  return null;
}

/** 글과 면의 짝: [앞, 뒤, 최소 대비, 어디서 쓰이는지]. 글 4.5, 입력 경계·초점·그림 3. */
type Pair = readonly [fg: string, bg: string, min: number, why: string];

const TONES = ['ok', 'info', 'warn', 'bad'] as const;
const on = (fgs: readonly string[], bgs: readonly string[], min: number, why: string): Pair[] => fgs.flatMap((fg) => bgs.map((bg) => [fg, bg, min, why] as const));

const PAIRS: Pair[] = [
  // 본문 글(Page·Section·Item·표·입력 …이 놓이는 면: bg, surface, surface-2, 눌렀을 때의 surface-3)
  ...on(['text', 'text-2', 'text-3'], ['bg', 'surface', 'surface-2'], 4.5, '본문·흐린 글·도움말이 놓이는 면'),
  ...on(['text'], ['surface-3', 'accent-soft'], 4.5, 'Button.ghost:hover, Tabs a:hover / ::selection'),
  ...on(['text-2'], ['surface-3'], 4.5, 'Tabs .n(건수), Shell .close:hover, Button.ghost'),
  // 안내 상자(Callout, 처리 결과 안내)의 글
  ...on(['text', 'text-2'], [...TONES.map((t) => `${t}-soft`), 'muted-soft'], 4.5, 'Callout·Shell .flash 안의 글, Badge 기본 결(muted-soft)'),
  // 링크·강조색 글
  ...on(['accent-text'], ['bg', 'surface', 'surface-2', 'accent-soft'], 4.5, '링크, Tabs 현재 칸, Disclosure summary, Shell .link 현재 칸(accent-soft)'),
  ...on(['accent-text'], TONES.map((t) => `${t}-soft`), 4.5, '안내 상자 안의 링크'),
  ...on(['on-accent'], ['accent', 'accent-hover'], 4.5, 'primary 단추, 건너뛰기 링크, Step 현재 번호'),
  // 결의 색 글: 자기 -soft 면, 본문 면
  ...TONES.flatMap((t) => on([t], [`${t}-soft`], 4.5, `Badge·Shell .state·.count·Item 앞 아이콘(${t})`)),
  ...on(['muted'], ['muted-soft'], 4.5, 'Item 앞 아이콘(muted)'),
  ...on([...TONES, 'muted'], ['surface', 'surface-2', 'bg'], 4.5, 'Text tone, Dot·Callout 아이콘이 본문 면에 놓일 때'),
  // 위험 단추
  ...on(['on-danger'], ['danger', 'danger-hover'], 4.5, 'Button danger(묻는 창 안의 확인)'),
  ...on(['danger'], ['surface', 'surface-2', 'bad-soft'], 4.5, 'Button ghost-danger와 그 hover(bad-soft)'),
  // 입력 경계·초점·그림(3:1)
  ...on(['border-strong'], ['surface', 'bg', 'surface-2'], 3, '입력·단추 경계(field.css .control, Button .btn), Step 번호, Screen 틀'),
  ...on(['focus'], ['surface', 'bg', 'surface-2'], 3, ':focus-visible 고리(base.css)'),
  ...on(['accent'], ['surface', 'bg'], 3, '체크 상자·라디오(accent-color), Stat hover 테두리'),
  ...on([...TONES, 'accent'], ['surface-3'], 3, 'Progress의 채움(트랙 surface-3 위. 결이 없으면 accent)'),
  // 디자인 언어(결정 0062)의 면: 쪽빛 띠, 하늘(본문 위쪽 번짐), 번짐·치맛단(사이드바 아래쪽)
  ...on(['on-band', 'on-band-2'], ['band', 'band-2'], 4.5, 'Section 띠의 제목·곁글, Shell 지금 화면 표시, 좁은 화면 위 막대의 이름'),
  ...on(['band-line'], ['band', 'band-2'], 3, '띠 위의 아이콘(금선 색)과 띠 위에 놓인 것의 초점 고리(Shell 위 막대)'),
  ...on(['text', 'text-2', 'text-3', 'accent-text'], ['sky', 'wash'], 4.5, 'Page 머리(하늘 번짐 위), 사이드바 위쪽(번짐), Solo 바탕'),
  ...on(['text', 'text-2'], ['hem'], 4.5, '사이드바 아래쪽(치맛단)의 메뉴·단추 글'),
  ...on(['focus'], ['sky', 'wash', 'hem'], 3, '하늘·번짐·치맛단 위의 초점 고리'),
];

const THEMES: [string, Map<string, string>][] = [
  ['밝은 판', hexTokens(LIGHT)],
  ['어두운 판', hexTokens(DARK)],
];

describe('색 토큰: tokens.css를 읽어 계산한다', () => {
  it('어두운 판이 두 곳(@media와 data-theme="dark")에서 같다', () => {
    const a = [...DARK_MEDIA].sort(([x], [y]) => x.localeCompare(y));
    const b = [...DARK].sort(([x], [y]) => x.localeCompare(y));
    expect(a, '어두운 판 토큰 묶음이 둘이 어긋난다').toEqual(b);
  });

  it('밝은 판의 모든 색 토큰이 어두운 판에도 있다', () => {
    const light = hexTokens(LIGHT);
    const dark = hexTokens(DARK);
    const missing = [...light.keys()].filter((k) => !dark.has(k));
    const extra = [...dark.keys()].filter((k) => !light.has(k));
    expect({ 어두운_판에_없는_토큰: missing, 밝은_판에_없는_토큰: extra }).toEqual({ 어두운_판에_없는_토큰: [], 밝은_판에_없는_토큰: [] });
  });

  for (const [theme, map] of THEMES) {
    it(`${theme}: 글 4.5:1, 경계·초점·그림 3:1(WCAG AA)`, () => {
      const lines: string[] = [];
      const need = (name: string) => {
        const v = map.get(name);
        if (!v) lines.push(`--${name}: 이 판에 6자리 색으로 정의되어 있지 않다`);
        return v;
      };
      for (const [fg, bg, min, why] of PAIRS) {
        const f = need(fg);
        const b = need(bg);
        if (!f || !b) continue;
        const r = ratio(f, b);
        if (r >= min) continue;
        // 고칠 값 제안: 앞 색을 그 앞 색의 다른 짝까지 모두 통과하는 가장 가까운 값으로, 또는 뒤 색만 옮겨서.
        const others = PAIRS.filter(([x]) => x === fg).flatMap(([, y, m]) => (map.get(y) ? [{ other: map.get(y)!, min: m }] : []));
        const towardFg = luminance(f) > luminance(b) ? '#ffffff' : '#000000';
        const fixFg = nudge(f, towardFg, others);
        const onlyThisBg = nudge(b, luminance(b) > luminance(f) ? '#ffffff' : '#000000', [{ other: f, min }]);
        lines.push(
          `--${fg}(${f}) 대 --${bg}(${b}): 대비 ${r.toFixed(2)} < ${min}  [${why}]` +
            `\n      제안: --${fg}를 ${fixFg ?? '(같은 방향으로는 안 된다)'}로(이 판에서 --${fg}의 모든 짝 통과)` +
            ` / 또는 --${bg}만 ${onlyThisBg ?? '(안 된다)'}로(이 짝만 통과, 다른 곳에 쓰이면 따로 확인)`,
        );
      }
      if (lines.length > 0) throw new Error(`${theme} 대비 부족 ${lines.length}건\n  - ${lines.join('\n  - ')}`);
    });
  }
});

// ───────────────────────── 3. 스타일 ─────────────────────────

const UI_DIR = join(ROOT, 'src/ui');
const styleOfSvelte = (src: string) => [...src.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)].map((m) => m[1]).join('\n');

/** 스타일 파일(base.css, field.css, tokens.css)과 부품(.svelte)의 <style>. */
const SHEETS: Record<string, CssRule[]> = Object.fromEntries(
  readdirSync(UI_DIR)
    .filter((f) => /\.(css|svelte)$/.test(f))
    .map((f): [string, CssRule[]] => {
      const text = readFileSync(join(UI_DIR, f), 'utf8');
      return [f, parseCss(f.endsWith('.svelte') ? styleOfSvelte(text) : text)];
    }),
);
// 파일만 읽는다(문양 그림은 하위 디렉터리 motif/에 있다).
const SOURCES: Record<string, string> = Object.fromEntries(
  readdirSync(UI_DIR, { withFileTypes: true })
    .filter((f) => f.isFile())
    .map((f): [string, string] => [f.name, readFileSync(join(UI_DIR, f.name), 'utf8')]),
);
const LENGTHS = new Map([...LIGHT]);

const selectorsOf = (r: CssRule) => r.selector.split(',').map((s) => norm(s));
const findRules = (file: string, selector: string, at: RegExp | null = null) =>
  (SHEETS[file] ?? []).filter((r) => selectorsOf(r).includes(selector) && (at ? at.test(r.at) : r.at === ''));

/** 길이 값을 px로(rem=16px, `var(--x)`는 tokens.css의 값). 못 읽으면 NaN. */
function px(value: string | undefined, depth = 0): number {
  if (!value) return NaN;
  const v = value.replace(/\s*!important/, '').trim();
  const variable = /^var\((--[a-z0-9-]+)\)$/.exec(v);
  if (variable && depth < 3) return px(LENGTHS.get(variable[1]), depth + 1);
  const m = /^(-?\d*\.?\d+)(px|rem)$/.exec(v);
  if (!m) return NaN;
  return Number(m[1]) * (m[2] === 'rem' ? 16 : 1);
}

const prop = (rules: CssRule[], name: string) => rules.map((r) => r.decl.get(name)).filter((v): v is string => v !== undefined).pop();

describe('스타일: 접근성 장치가 남아 있다', () => {
  it(':focus-visible 고리: 3px 이상, 초점 색 토큰', () => {
    const rules = findRules('base.css', ':focus-visible');
    expect(rules.length, 'base.css에 :focus-visible 규칙이 없다').toBeGreaterThan(0);
    const outline = prop(rules, 'outline') ?? '';
    const width = px(/(-?\d*\.?\d+(?:px|rem))/.exec(outline)?.[1]);
    expect(width, `:focus-visible의 outline이 3px 이상이어야 한다: "${outline}"`).toBeGreaterThanOrEqual(3);
    expect(outline, ':focus-visible 고리는 --focus 토큰을 쓴다').toContain('var(--focus)');
    expect(outline, ':focus-visible 고리가 실선이어야 한다').not.toMatch(/\b(none|hidden)\b/);
  });

  it('부품이 초점 고리를 지우거나 줄이지 않는다(main의 프로그램 초점 제외)', () => {
    const bad: string[] = [];
    for (const [file, rules] of Object.entries(SHEETS)) {
      for (const r of rules) {
        const outline = r.decl.get('outline') ?? r.decl.get('outline-style') ?? '';
        const removes = /^(none|0|hidden)\b/.test(outline) || r.decl.get('outline-width') === '0';
        // 쪽의 큰 틀(main, tabindex=-1)은 건너뛰기 링크가 옮겨 오는 곳이라 고리를 그리지 않는다.
        if (removes && !selectorsOf(r).every((s) => s === 'main')) bad.push(`${file}: ${r.selector} { outline: ${outline} }`);
        if (r.selector.includes(':focus') && outline && !removes) {
          const w = px(/(-?\d*\.?\d+(?:px|rem))/.exec(outline)?.[1]);
          if (w < 3) bad.push(`${file}: ${r.selector} { outline: ${outline} } (3px 미만)`);
        }
      }
    }
    expect(bad, '초점 고리를 없애거나 줄이는 규칙').toEqual([]);
  });

  it('prefers-reduced-motion: 움직임을 거의 끈다', () => {
    const rules = findRules('base.css', '*', /prefers-reduced-motion:\s*reduce/);
    expect(rules.length, 'base.css에 @media (prefers-reduced-motion: reduce) { * … } 가 없다').toBeGreaterThan(0);
    const d = rules[0].decl;
    expect(d.get('animation-duration'), 'animation-duration').toMatch(/^0(\.0+\d*)?(ms|s)/);
    expect(d.get('transition-duration'), 'transition-duration').toMatch(/^0(\.0+\d*)?(ms|s)/);
    expect(d.get('animation-iteration-count'), 'animation-iteration-count').toMatch(/^1\b/);
    expect(d.get('scroll-behavior'), 'scroll-behavior').toMatch(/^auto\b/);
    // 부품이 쓰는 transition·animation은 이 전역 규칙이 모두 덮는다(*, ::before, ::after).
    expect(selectorsOf(rules[0]).length, '*, *::before, *::after').toBeGreaterThanOrEqual(3);
  });

  it('forced-colors: 배지·단추는 경계를 시스템 색으로 남긴다', () => {
    for (const file of ['Badge.svelte', 'Button.svelte']) {
      const rules = (SHEETS[file] ?? []).filter((r) => /forced-colors:\s*active/.test(r.at));
      expect(rules.length, `${file}에 @media (forced-colors: active)가 없다`).toBeGreaterThan(0);
      expect(
        rules.some((r) => [...r.decl.keys()].some((k) => k.startsWith('border'))),
        `${file}: forced-colors에서 border를 정하지 않는다(배경색이 사라지면 모양이 없어진다)`,
      ).toBe(true);
    }
  });

  it('forced-colors: 현재 화면·현재 칸 표시가 배경색 없이도 남는다(예전 스타일시트가 하던 것)', () => {
    // 현재 칸을 배경색과 굵기로만 알리면 강제 색 모드에서 사라진다. 예전에는 outline 2px Highlight를 더했다.
    const missing: string[] = [];
    for (const file of ['Shell.svelte', 'Tabs.svelte']) {
      const rules = (SHEETS[file] ?? []).filter((r) => /forced-colors:\s*active/.test(r.at) && r.selector.includes('aria-current'));
      const ok = rules.some((r) => ['outline', 'border', 'text-decoration', 'forced-color-adjust'].some((k) => [...r.decl.keys()].some((d) => d.startsWith(k))));
      if (!ok) missing.push(`${file}: @media (forced-colors: active) { …[aria-current='page'] { outline: 2px solid Highlight } } 같은 규칙이 없다`);
    }
    expect(missing, '현재 위치 표시가 강제 색 모드에서 사라진다').toEqual([]);
  });

  it('pointer: coarse: 누를 곳의 크기와 입력 글자 크기', () => {
    const coarse = /pointer:\s*coarse/;
    const bad: string[] = [];
    // [파일, 선택자, 거친 포인터에서의 min-height 하한(px), 까닭]. 예전 시험의 기준: 단추 44, 작은 단추·칩 40.
    const targets: [string, string, number, string][] = [
      ['Button.svelte', '.btn', 44, '단추'],
      ['Button.svelte', '.sm', 40, '작은 단추'],
      ['field.css', '.field .control', 44, '입력·고르기'],
      ['CheckField.svelte', 'label', 44, '체크 상자 줄'],
      ['Disclosure.svelte', 'summary', 44, '펼침 제목'],
      ['Tabs.svelte', 'a', 40, '탭 링크'],
    ];
    for (const [file, selector, min, why] of targets) {
      const got = px(prop(findRules(file, selector, coarse), 'min-height'));
      if (!(got >= min)) bad.push(`${file} ${selector}(${why}): pointer: coarse에서 min-height ${got || '없음'}px < ${min}px`);
    }
    // 입력 글자가 16px보다 작으면 iOS가 화면을 확대한다.
    const font = px(prop(findRules('base.css', 'input', coarse), 'font-size'));
    if (!(font >= 16)) bad.push(`base.css @media (pointer: coarse) { input, select, textarea { font-size } }가 16px 이상이 아니다(${font || '없음'})`);
    expect(bad, '터치 화면에서 누를 곳이 작다').toEqual([]);
  });

  it('누를 곳은 어느 포인터에서도 24px 이상이다(WCAG 2.5.8)', () => {
    const bad: string[] = [];
    const targets: [string, string][] = [
      ['Button.svelte', '.btn'],
      ['Button.svelte', '.sm'],
      ['field.css', '.field .control'],
      ['CheckField.svelte', 'label'],
      ['Disclosure.svelte', 'summary'],
      ['Tabs.svelte', 'a'],
      ['Shell.svelte', '.link'],
      ['Shell.svelte', '.close'],
      ['Page.svelte', '.back'],
    ];
    for (const [file, selector] of targets) {
      const r = findRules(file, selector);
      const h = px(prop(r, 'min-height') ?? prop(r, 'height'));
      if (!(h >= 24)) bad.push(`${file} ${selector}: 기본 높이 ${h || '정해지지 않음'}px < 24px`);
    }
    expect(bad, '누를 곳이 너무 작다').toEqual([]);
  });

  it('.sr-only: 화면에서만 숨기고 보조 기술에는 남긴다', () => {
    const r = findRules('base.css', '.sr-only');
    expect(r.length, 'base.css에 .sr-only가 없다').toBeGreaterThan(0);
    const get = (name: string) => prop(r, name);
    expect(get('position'), 'position').toBe('absolute');
    expect(px(get('width')), 'width').toBe(1);
    expect(px(get('height')), 'height').toBe(1);
    expect(get('overflow'), 'overflow').toBe('hidden');
    expect(get('clip-path') ?? get('clip'), 'clip-path나 clip').toMatch(/inset\(50%\)|rect\(0/);
    expect(get('white-space'), 'white-space').toBe('nowrap');
    expect(get('display'), 'display:none은 보조 기술에서도 지운다').not.toBe('none');
    expect(get('visibility'), 'visibility:hidden은 보조 기술에서도 지운다').not.toBe('hidden');
  });

  it('좁은 표에서 머리 줄을 display:none으로 지우지 않는다', () => {
    const thead = (SHEETS['Table.svelte'] ?? []).filter((r) => selectorsOf(r).includes('thead'));
    for (const r of thead) {
      expect(r.decl.get('display'), `Table.svelte thead(${r.at})`).not.toBe('none');
      expect(r.decl.get('visibility'), `Table.svelte thead(${r.at})`).not.toBe('hidden');
    }
  });

  it('인쇄: 바탕·글 색을 맞추고 틀(사이드바·위 막대·아래 막대·건너뛰기)을 숨긴다', () => {
    const body = findRules('base.css', 'body', /^@media print$/);
    expect(body.length, 'base.css에 @media print { body } 가 없다').toBeGreaterThan(0);
    expect(prop(body, 'background'), '인쇄 바탕').toBeTruthy();
    expect(prop(body, 'color'), '인쇄 글 색').toBeTruthy();
    const hidden = (SHEETS['Shell.svelte'] ?? []).filter((r) => r.at === '@media print' && r.decl.get('display') === 'none').flatMap(selectorsOf);
    for (const sel of ['.side', '.top', '.bottom', '.skip']) {
      expect(hidden, `Shell.svelte @media print에서 ${sel}를 숨겨야 한다`).toContain(sel);
    }
  });

  it('건너뛰기 링크는 초점이 오면 보이고, 쪽 안 이동은 위 막대에 가리지 않는다', () => {
    const skip = findRules('Shell.svelte', '.skip');
    const focus = findRules('Shell.svelte', '.skip:focus');
    expect(skip.length, 'Shell.svelte에 .skip 규칙이 없다').toBeGreaterThan(0);
    expect(focus.length, 'Shell.svelte에 .skip:focus 규칙이 없다').toBeGreaterThan(0);
    expect(px(prop(skip, 'top')), '.skip은 화면 밖(top < 0)에 있다가').toBeLessThan(0);
    expect(px(prop(focus, 'top')), '.skip:focus는 화면 안(top ≥ 0)으로 온다').toBeGreaterThanOrEqual(0);
    // 위에 붙는 막대(.top, 약 3.1rem)에 가리지 않게 이동할 곳 위에 여백을 둔다.
    const html = findRules('base.css', 'html');
    expect(px(prop(html, 'scroll-padding-top')), 'base.css html { scroll-padding-top }(위 막대 높이보다 커야 한다)').toBeGreaterThanOrEqual(56);
    expect(px(prop(findRules('Section.svelte', 'section'), 'scroll-margin-top')), 'Section { scroll-margin-top }').toBeGreaterThanOrEqual(56);
  });

  it('화면 넘겨받기 그림: 줄어들어도 누른 자리가 원래 그림의 좌표로 간다(예전의 "그림을 줄이지 않는다"를 대신한다)', () => {
    // 예전에는 그림이 줄지 않아야 좌표가 맞았다(CSS 규칙). 지금은 그림이 자리에 맞춰 줄고, 보낼 때 원래 크기로 환산한다.
    const src = SOURCES['Screen.svelte'];
    expect(src, 'Screen.svelte가 naturalWidth로 좌표를 환산해야 한다').toContain('naturalWidth');
    expect(src, 'Screen.svelte가 naturalHeight로 좌표를 환산해야 한다').toContain('naturalHeight');
    expect(src, 'Screen.svelte가 누른 좌표를 at.x·at.y로 보낸다').toMatch(/'at\.x'[\s\S]*'at\.y'/);
  });

  it('밝은·어두운 판을 UA 부품(입력, 스크롤 막대)도 따른다(color-scheme)', () => {
    const html = findRules('base.css', 'html');
    expect(prop(html, 'color-scheme'), 'base.css html { color-scheme }').toMatch(/light\s+dark/);
    expect(prop(findRules('base.css', "html[data-theme='light']"), 'color-scheme'), "html[data-theme='light']").toBe('light');
    expect(prop(findRules('base.css', "html[data-theme='dark']"), 'color-scheme'), "html[data-theme='dark']").toBe('dark');
  });
});

describe('스타일: 색은 토큰으로 정한다', () => {
  // 밝은·어두운 판이 따로 있는 색은 토큰으로만 쓴다. 판을 따르지 않는 고정 색은 아래 목록(까닭 있는 것)만 허용한다.
  const ALLOWED = new Set(['Shell.svelte|.mark', 'Solo.svelte|.mark', 'base.css|body@print']);

  it('고정 색(#rrggbb, rgb())은 허용한 자리에만 있다', () => {
    const bad: string[] = [];
    for (const [file, rules] of Object.entries(SHEETS)) {
      if (file === 'tokens.css') continue;
      for (const r of rules) {
        const key = `${file}|${r.selector}${r.at === '@media print' ? '@print' : ''}`;
        for (const [name, value] of r.decl) {
          if (/#[0-9a-fA-F]{3,8}\b|\brgba?\(/.test(value) && !ALLOWED.has(key)) bad.push(`${file}: ${r.selector} { ${name}: ${value} }`);
        }
      }
    }
    expect(bad, '고정 색(어두운 판에서 바뀌지 않는다). 토큰을 쓰거나 이 시험의 허용 목록에 까닭과 함께 더한다').toEqual([]);
  });

  it('브랜드 표식(.mark)의 아이콘은 그라데이션 양 끝과 3:1 이상이다', () => {
    const bad: string[] = [];
    for (const file of ['Shell.svelte', 'Solo.svelte']) {
      const r = findRules(file, '.mark').find((x) => x.decl.get('background')?.includes('gradient'));
      if (!r) {
        bad.push(`${file}: .mark의 그라데이션 규칙을 찾지 못했다`);
        continue;
      }
      const stops = [...(r.decl.get('background') ?? '').matchAll(/#[0-9a-fA-F]{6}\b/g)].map((m) => m[0].toLowerCase());
      const fg = r.decl.get('color')?.toLowerCase();
      const full = fg === '#fff' ? '#ffffff' : fg;
      if (!full || !HEX.test(full) || stops.length === 0) {
        bad.push(`${file}: .mark의 색을 읽지 못했다(color: ${fg}, 그라데이션 ${stops.length}개)`);
        continue;
      }
      for (const stop of stops) {
        const rr = ratio(full, stop);
        if (rr < 3) bad.push(`${file} .mark: ${full} 대 ${stop} 대비 ${rr.toFixed(2)} < 3`);
      }
    }
    expect(bad, '브랜드 표식 아이콘 대비').toEqual([]);
  });
});

// ───────────────────────── 4. 이 시험 자체 ─────────────────────────
// 위의 규칙이 실제로 위반을 잡는지(아무것도 못 잡아서 늘 통과하는 일이 없도록) 일부러 틀린 마크업·스타일에 돌려 본다.

describe('이 시험이 위반을 실제로 잡는다', () => {
  const page = { kind: 'page', page: 'overview', flash: { tone: 'ok', text: '안내' } } as unknown as PageData;
  const hitsOf = (id: string, html: string, bad = false, data: PageData | StandaloneData = page) => {
    document.body.innerHTML = html;
    document.title = bad && id === 'document-title' ? '' : '제목';
    const rule = MARKUP_RULES.find((r) => r.id === id);
    if (!rule) throw new Error(`규칙 없음: ${id}`);
    return rule.run({ root: document.body, doc: document, data });
  };

  /** 규칙 → [위반이 있는 마크업들]. 마크업마다 위반이 하나 이상 나와야 한다. */
  const BAD: Record<string, string[]> = {
    'h1-one': ['<main id="main"></main>', '<main id="main"><h1>가</h1><h1>나</h1></main>'],
    'main-one': ['<div></div>', '<main></main>', '<main id="main"></main><main></main>'],
    'skip-link': ['<button>가</button><main id="main"></main>', '<button>가</button><a href="#main">건너뛰기</a>'],
    'nav-name': ['<nav><a href="/">홈</a></nav>', '<nav aria-label="가"></nav><nav aria-label="가"></nav>'],
    'aria-current': [
      '<nav aria-label="가"><a href="/work" aria-current="page">가</a><a href="/" aria-current="page">나</a></nav>',
      '<nav aria-label="가"><a href="/">홈</a><a href="/work" aria-current="page">할 일</a></nav>',
      '<nav aria-label="가"><a href="/">홈</a><a href="/work">할 일</a></nav>',
      '<nav aria-label="가"><a href="/" aria-current="yes">홈</a></nav>',
    ],
    'heading-order': ['<h1>가</h1><h3>나</h3>', '<h1>가</h1><h2>나</h2><h3>다</h3><h2>라</h2><h4>마</h4>', '<h1>가</h1><dialog><h3>나</h3></dialog>', '<h1>가</h1><h2></h2>'],
    'id-unique': ['<i id="a"></i><b id="a"></b>'],
    'refs-resolve': ['<label for="없음">가</label>', '<input id="a" aria-describedby="없음">', '<a href="#없음">가</a>', '<label for="p">가</label><p id="p"></p>', '<a href="#">가</a>'],
    'control-label': ['<input type="text">', '<select></select>', '<textarea></textarea>', '<input id="a"><label for="a"></label>', '<input type="submit">'],
    'interactive-text': ['<button></button>', '<button><svg aria-hidden="true"></svg></button>', '<a href="/"><svg aria-hidden="true"></svg></a>', '<details><summary></summary></details>'],
    'widget-name': ['<dialog></dialog>', '<progress></progress>'],
    'details-summary': ['<details><p>가</p></details>', '<details><summary>가</summary><summary>나</summary></details>'],
    'img-alt': ['<img src="x">', '<input type="image" alt="">'],
    'table-scope': ['<table><tr><th>가</th></tr></table>', '<table><tr><td>가</td></tr></table>'],
    'icon-hidden': ['<svg></svg>'],
    'dot-badge-text': ['<p><span class="dot"></span></p>', '<p><span class="dot" aria-hidden="true"></span><span class="sr-only">가</span></p>', '<span class="badge"><svg aria-hidden="true"></svg></span>'],
    'aria-hidden-focus': ['<div aria-hidden="true"><a href="/">가</a></div>'],
    'flash-role': ['<main><div><p>안내</p></div></main>'],
    'document-title': ['<main></main>'],
  };

  /** 틀리지 않은 마크업 하나(모든 규칙이 위반 없이 지나가야 한다). */
  const GOOD = `
    <a href="#main">본문으로 건너뛰기</a>
    <nav aria-label="주 메뉴"><a href="/" aria-current="page">홈</a><a href="/work">할 일</a></nav>
    <nav aria-label="빠른 메뉴"><a href="/" aria-current="page"><svg aria-hidden="true"></svg><span>홈</span><span class="pip" aria-hidden="true"></span></a><a href="/work">할 일</a></nav>
    <main id="main">
      <div role="status"><p>안내</p></div>
      <h1>제목</h1><h2>구역</h2><h3>작은 제목</h3><h2 id="b">다른 구역</h2>
      <p><span class="dot" aria-hidden="true"></span><span>정상</span></p>
      <span class="badge"><svg aria-hidden="true"></svg><span>정상</span></span>
      <label for="a">이름</label><input id="a" aria-describedby="h"><span id="h">도움말</span>
      <label>동의 <input type="checkbox"></label>
      <input type="hidden" name="x"><input type="image" alt="화면" src="x"><img src="x" alt="">
      <button><svg aria-hidden="true"></svg><span>저장</span></button><button aria-label="닫기"><svg aria-hidden="true"></svg></button>
      <details><summary>자세히</summary><p>글</p></details>
      <table><thead><tr><th scope="col">이름</th></tr></thead><tbody><tr><td>값</td></tr></tbody></table>
      <progress aria-label="사용량"></progress><a href="#b">구역으로</a>
      <dialog aria-labelledby="t"><h2 id="t">묻는 창</h2><h3>더</h3></dialog>
    </main>`;

  for (const [id, list] of Object.entries(BAD)) {
    it(`${id}: 틀린 마크업을 잡는다`, () => {
      list.forEach((html, i) => expect(hitsOf(id, html, true).length, `${id} 틀린 마크업 #${i + 1}이 안 잡힌다: ${html}`).toBeGreaterThan(0));
    });
  }

  it('모든 규칙에 틀린 마크업 시험이 있다', () => {
    expect(MARKUP_RULES.map((r) => r.id).filter((id) => !BAD[id])).toEqual([]);
  });

  it('틀리지 않은 마크업은 모든 규칙을 지난다', () => {
    const all = Object.fromEntries(MARKUP_RULES.map((r) => [r.id, hitsOf(r.id, GOOD)]));
    expect(all).toEqual(Object.fromEntries(MARKUP_RULES.map((r) => [r.id, []])));
  });

  it('대비 계산: 알려진 값과 맞는다', () => {
    expect(ratio('#000000', '#ffffff')).toBeCloseTo(21, 1);
    expect(ratio('#777777', '#ffffff')).toBeCloseTo(4.48, 1); // 잘 알려진 "AA에 아슬아슬하게 모자란" 회색
    expect(ratio('#767676', '#ffffff')).toBeGreaterThanOrEqual(4.5);
    expect(ratio('#ffffff', '#000000')).toBeCloseTo(ratio('#000000', '#ffffff'), 5);
  });

  it('스타일 읽기: 중첩된 @media와 var() 길이를 읽는다', () => {
    const rules = parseCss('a, b { min-height: var(--control-h); }\n@media (pointer: coarse) { /* 주석 */ a { min-height: 2.75rem !important; } }\n@keyframes k { from { top: 0 } }');
    expect(rules.map((r) => [r.at, r.selector])).toEqual([
      ['', 'a, b'],
      ['@media (pointer: coarse)', 'a'],
    ]);
    expect(px(rules[0].decl.get('min-height'))).toBe(40);
    expect(px(rules[1].decl.get('min-height'))).toBe(44);
    expect(px('3px')).toBe(3);
    expect(px('auto')).toBeNaN();
  });

  it('고칠 값 제안은 그 색의 모든 짝을 통과한다', () => {
    const fixed = nudge('#868e9c', '#000000', [
      { other: '#ffffff', min: 3 },
      { other: '#f0f2f5', min: 3 },
    ]);
    expect(fixed).not.toBeNull();
    expect(ratio(fixed!, '#f0f2f5')).toBeGreaterThanOrEqual(3);
  });
});
