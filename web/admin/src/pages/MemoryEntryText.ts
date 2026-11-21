// 기억 항목의 세부 정보를 그릴 조각으로 나눈다(예전 화면 코드의 `memory_text`와 같은 규칙).
// `- `로 시작하는 줄은 목록(이어지면 한 목록), `#`으로 시작하면 소제목, 나머지는 문단. 빈 줄은 건너뛴다.
// 줄 안의 `[[이름]]`은 이름이 영문 소문자·숫자·`-`로만 이루어졌을 때 그 항목으로 잇는 링크가 된다.

/** 글 한 조각. `link`이면 `text`가 항목 이름이다. */
export interface Inline {
  link: boolean;
  text: string;
}

export type Block =
  | { kind: 'list'; items: Inline[][] }
  | { kind: 'heading'; content: Inline[] }
  | { kind: 'para'; content: Inline[] };

function inline(line: string): Inline[] {
  const parts: Inline[] = [];
  let rest = line;
  for (;;) {
    const start = rest.indexOf('[[');
    if (start < 0) break;
    const after = rest.slice(start + 2);
    const end = after.indexOf(']]');
    if (end < 0) break; // 닫는 `]]`가 없으면 그 뒤는 글 그대로다.
    const name = after.slice(0, end);
    if (!/^[a-z0-9-]+$/.test(name)) {
      // 이름이 아니면 `[[`는 글로 남기고 그 뒤부터 다시 찾는다.
      parts.push({ link: false, text: rest.slice(0, start + 2) });
      rest = after;
      continue;
    }
    parts.push({ link: false, text: rest.slice(0, start) });
    parts.push({ link: true, text: name });
    rest = after.slice(end + 2);
  }
  parts.push({ link: false, text: rest });
  return parts;
}

export function parseMemoryText(text: string): Block[] {
  const blocks: Block[] = [];
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (line.startsWith('- ')) {
      const item = inline(line.slice(2));
      const last = blocks.at(-1);
      if (last?.kind === 'list') last.items.push(item);
      else blocks.push({ kind: 'list', items: [item] });
    } else if (line.startsWith('#')) {
      blocks.push({ kind: 'heading', content: inline(line.replace(/^#+/, '').trim()) });
    } else if (line !== '') {
      blocks.push({ kind: 'para', content: inline(line) });
    }
  }
  return blocks;
}
