// 화면 코드의 규칙(결정 0061). 화면(src/pages, src/standalone)은 **부품만으로** 짠다: 스타일과 틀은 src/ui에만 있다.
// 기능을 더할 때마다 화면마다 모양이 달라지던 일을 여기서 막는다. 부품이 모자라면 화면에서 때우지 말고 부품을 고친다.
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { describe, expect, it } from 'vitest';
import { ROOT } from './helpers';

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });
}

const screens = [...files(join(ROOT, 'src/pages')), ...files(join(ROOT, 'src/standalone'))].filter((f) =>
  /\.(svelte|ts)$/.test(f),
);

/** [정규식, 까닭]. 주석은 지우고 본다. */
const RULES: [RegExp, string][] = [
  [/<style[\s>]/, '화면에 <style>을 두지 않는다(스타일은 src/ui의 부품에만)'],
  [/\sstyle\s*=|\sstyle:/, '인라인 스타일을 쓰지 않는다(CSP가 막는다. 배치는 Stack·Cluster·Grid로)'],
  [/\sclass\s*=|\sclass:/, '화면에서 class를 쓰지 않는다(모양은 부품의 속성으로 정한다)'],
  [/\{@html\b/, '{@html}을 쓰지 않는다(바깥 글이 마크업으로 들어가면 안 된다)'],
  [/\bfetch\s*\(|XMLHttpRequest|navigator\.sendBeacon/, '서버 요청은 ActionForm과 lib/app이 한다'],
  [/<form[\s>]/, '<form> 대신 ActionForm(또는 PostButton·Confirm)을 쓴다'],
  [/<(button|select|textarea|table|dialog|section|details|h1|h2|nav|main|header|footer)[\s>]/, '이 태그는 부품이 그린다(Button, SelectField, TextArea, Table, Confirm, Section, Disclosure, Page, Tabs …)'],
  [/<input(?![^>]*type=["']hidden["'])[\s>]/, '<input>은 type="hidden"만 직접 쓴다(나머지는 TextField·CheckField …)'],
  [/\bon(submit|input|change)\s*=/, '폼의 처리는 ActionForm이 한다'],
  [/innerHTML|outerHTML|insertAdjacentHTML|document\.write/, 'DOM에 마크업을 직접 넣지 않는다'],
  [/localStorage|sessionStorage|document\.cookie/, '브라우저 저장소를 화면에서 쓰지 않는다'],
  [/\beval\s*\(|new Function\s*\(/, 'CSP가 막는다'],
  [/\$app\b|window\.location\.(assign|replace|href\s*=)/, '화면 전환은 링크(<a href>)와 lib/app의 navigate로'],
];

const ALLOWED_IMPORT = /^(svelte(\/.*)?|\.\.\/ui|\.\.\/lib\/[A-Za-z0-9_.-]+|\.\/[A-Za-z0-9_./-]+)$/;

describe('화면은 부품만으로 짠다', () => {
  for (const file of screens) {
    it(relative(ROOT, file), () => {
      const text = readFileSync(file, 'utf8')
        .replace(/<!--[\s\S]*?-->/g, '')
        .replace(/\/\*[\s\S]*?\*\//g, '')
        .replace(/(^|[^:])\/\/.*$/gm, '$1');
      const broken: string[] = [];
      for (const [re, why] of RULES) {
        const m = re.exec(text);
        if (m) broken.push(`${why} — "${m[0].trim()}"`);
      }
      for (const m of text.matchAll(/from\s+['"]([^'"]+)['"]/g)) {
        if (!ALLOWED_IMPORT.test(m[1])) broken.push(`가져올 수 없는 곳: ${m[1]} (부품은 '../ui'에서만)`);
      }
      expect(broken).toEqual([]);
    });
  }
});
