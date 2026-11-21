// 화면 쪽 시험의 공통 도구: 시험 데이터 읽기, 화면 그리기, 화면에서 "쓸 수 있는 것"의 목록 뽑기.
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { flushSync, mount, unmount } from 'svelte';
import App from '../src/App.svelte';
import { app } from '../src/lib/app.svelte';
import type { PageData, StandaloneData } from '../src/lib/types';

export const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const fx = (path: string) => JSON.parse(readFileSync(join(ROOT, 'fixtures', path), 'utf8'));

/** 상태 묶음. `stress`는 길고 많은 데이터(배치 검사용)라 기능 목록에는 쓰지 않는다. */
export const SETS = ['healthy', 'fresh', 'troubled', 'full', 'alt', 'open'] as const;

export interface FixturePage {
  set: string;
  /** 개편 전 화면 목록의 이름(예: "/work", "memory/entry", "handover/active"). */
  name: string;
  url: string;
  status: number;
  data: PageData;
}

const cache = new Map<string, FixturePage[]>();

/** 한 묶음의 모든 화면(하위 화면 포함). */
export function pages(set: string): FixturePage[] {
  let list = cache.get(set);
  if (!list) {
    const shared = fx(`sets/${set}.json`);
    list = (fx(`pages/${set}.json`) as { name: string; url: string; status: number; data: object }[]).map((p) => ({
      set,
      name: p.name,
      url: p.url,
      status: p.status,
      data: { ...shared, ...p.data } as PageData,
    }));
    cache.set(set, list);
  }
  return list;
}

export function page(set: string, name: string): FixturePage {
  const found = pages(set).find((p) => p.name === name);
  if (!found) throw new Error(`시험 데이터에 없는 화면: ${set}:${name}`);
  return found;
}

export interface FixtureScreen {
  name: string;
  url: string;
  data: StandaloneData;
}

/** 로그인 없이 여는 화면들. */
export function screens(): FixtureScreen[] {
  const index = fx('index.json') as { standalone: { name: string; url: string; file: string }[] };
  return index.standalone.map((s) => ({ name: s.name, url: s.url, data: fx(s.file) as StandaloneData }));
}

/** 화면마다 개편 전에 있던 줄(폼, 입력, 링크, 표지). */
export function baselineByPage(): Record<string, string[]> {
  return fx('baseline-by-page.json');
}

/** 개편 전 화면 전체에서 뽑아 둔 기준 목록(결정 0041). */
export function baseline(): string[] {
  return readFileSync(join(ROOT, 'tests/parity_baseline.txt'), 'utf8')
    .split('\n')
    .filter((l) => l.length > 0);
}

let mounted: { component: ReturnType<typeof mount>; target: HTMLElement } | null = null;

/** 화면 하나를 그린다(틀 포함). 돌려주는 것은 문서의 body다. 다음에 그리면 앞의 것은 치운다. */
export function render(data: PageData | StandaloneData, url = '/'): HTMLElement {
  clear();
  window.history.replaceState(null, '', url);
  app.data = structuredClone(data);
  app.flash = data.kind === 'page' ? data.flash : null;
  app.error = null;
  app.loading = false;
  app.url = url;
  app.version++;
  const target = document.createElement('div');
  document.body.append(target);
  const component = mount(App, { target });
  flushSync();
  mounted = { component, target };
  return document.body;
}

export function clear() {
  if (mounted) {
    void unmount(mounted.component);
    mounted.target.remove();
    mounted = null;
  }
  document.body.innerHTML = '';
}

/** 값이 상태나 데이터라서 목록에 넣지 않는 입력 종류. */
const SKIP_VALUE_FOR = ['text', 'password', 'url', 'number', 'file', 'search', ''];
const CTL_ATTRS = [
  'min',
  'max',
  'minlength',
  'maxlength',
  'step',
  'required',
  'inputmode',
  'autocomplete',
  'accept',
  'pattern',
  'rows',
  'disabled',
];

/**
 * 화면에서 폼·입력·고르기 값·링크·표지를 줄 목록으로 뽑는다. 데몬에 있던 예전 구현(결정 0041)을 옮긴 것이다.
 * 옮길 때 개편 전 화면의 HTML 50종에서 예전 구현과 같은 줄을 내는지 맞춰 봤다(결정 0061).
 * 값은 속성이 아니라 실제로 보내질 값(DOM의 value)에서 읽는다.
 */
export function inventory(root: ParentNode): Set<string> {
  const out = new Set<string>();
  for (const form of root.querySelectorAll('form')) {
    const action = form.getAttribute('action') ?? '';
    let head = `form ${form.getAttribute('method') ?? 'get'} ${action}`;
    const enctype = form.getAttribute('enctype');
    if (enctype !== null) head += ` enctype=${enctype}`;
    out.add(head);
    for (const el of form.querySelectorAll<HTMLElement>('input, select, textarea, button, option')) {
      const tag = el.tagName.toLowerCase();
      if (tag === 'option') {
        const select = el.closest('select');
        const option = el as HTMLOptionElement;
        if (el.hasAttribute('value') || '__value' in el) {
          out.add(`opt ${action} ${select?.getAttribute('name') ?? ''} ${option.value}`);
        }
        continue;
      }
      const name = el.getAttribute('name') ?? '';
      if (name === 'csrf' || name === 'back') continue;
      const type = el.getAttribute('type') ?? '';
      let line = `ctl ${action} ${tag}[${type}] name=${name}`;
      for (const key of CTL_ATTRS) {
        const v = el.getAttribute(key);
        if (v !== null) line += ` ${key}=${v}`;
      }
      if (!SKIP_VALUE_FOR.includes(type) && tag !== 'textarea' && tag !== 'select') {
        const value = (el as HTMLInputElement | HTMLButtonElement).value;
        const implicit = (type === 'checkbox' || type === 'radio') && value === 'on';
        const has = el.hasAttribute('value') || (tag === 'input' && !implicit && (value !== '' || type === 'hidden'));
        if (has) line += ` value=${value}`;
      }
      if (type === 'image') {
        const src = el.getAttribute('src');
        if (src !== null) line += ` src=${src.split('?')[0]}`;
      }
      out.add(line);
    }
  }
  for (const a of root.querySelectorAll('a[href]')) {
    const href = a.getAttribute('href') ?? '';
    if (href.startsWith('/static/') || href.startsWith('static/')) continue;
    if (href.startsWith('/') || href.startsWith('http')) out.add(`link ${href}`);
  }
  for (const meta of root.querySelectorAll('meta[http-equiv="refresh"]')) {
    out.add(`refresh ${meta.getAttribute('content') ?? ''}`);
  }
  // 표지: 마크업에 보이는 것과 입력에 채워진 값(속성이 아니라 DOM 값으로 들어간다).
  let text = root instanceof Element ? root.outerHTML : (root as Document).documentElement.outerHTML;
  for (const el of root.querySelectorAll<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>(
    'input, textarea, select, option',
  )) {
    text += `\n${el.value}`;
  }
  for (const mark of text.match(/qz-[a-z0-9-]+/g) ?? []) out.add(`data ${mark}`);
  return out;
}
