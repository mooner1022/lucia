// 시험 데이터(fixtures)에서 서버가 주는 JSON의 꼴을 읽어 TypeScript 타입을 만든다: src/lib/types.gen.ts.
// 데몬의 데이터 꼴이 바뀌면 시험 데이터를 다시 만든 뒤 `npm run types`를 돌린다.
// 한계: 열거형 값은 본 것만 알 수 있어 string으로 둔다. 비어 있기만 했던 목록은 unknown[]이다.
// 비어 있을 수 있는 칸: 시험 데이터에 늘 값이 있었더라도, 데몬의 타입에서 그 이름의 칸이 늘 `Option`이면
// `| null`을 붙인다(이름으로 맞춘 어림이다. 실제 데이터에서 null을 만나 화면이 죽는 것을 줄인다).
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

function rustFiles(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? rustFiles(join(dir, e.name)) : e.name.endsWith('.rs') ? [join(dir, e.name)] : [],
  );
}
// 이름이 같은 칸이 어디서는 Option이고 어디서는 아니면 알 수 없으니, **어디서나 Option인 이름만** 고른다.
const optional = new Set();
const required = new Set();
for (const crate of ['lucia-store', 'lucia-channel', 'luciad']) {
  for (const file of rustFiles(join(root, '../../crates', crate, 'src'))) {
    for (const m of readFileSync(file, 'utf8').matchAll(/^\s*(?:pub(?:\([a-z: ]+\))? )?([a-z_][a-z0-9_]*): ([A-Za-z_&][^,\n]*),?$/gm)) {
      (m[2].startsWith('Option<') ? optional : required).add(m[1]);
    }
  }
}
const OPTIONAL = new Set([...optional].filter((name) => !required.has(name)));

const fx = join(root, 'fixtures');
const SETS = readdirSync(join(fx, 'sets'))
  .map((f) => f.replace(/\.json$/, ''))
  .filter((s) => s !== 'stress');

function shapeOf(v) {
  if (v === null || v === undefined) return { t: 'null' };
  if (Array.isArray(v)) {
    // 길이가 같고 자리마다 종류가 다른 짧은 배열은 짝(튜플)이다: [Watch, SourceState|null], [Client, bool].
    const kinds = v.map((x) => (x === null ? 'null' : Array.isArray(x) ? 'array' : typeof x));
    if (v.length >= 2 && v.length <= 3 && new Set(kinds).size > 1) return { t: 'tuple', items: v.map(shapeOf) };
    let of = null;
    for (const item of v) of = of ? merge(of, shapeOf(item)) : shapeOf(item);
    return { t: 'array', of };
  }
  if (typeof v === 'object') {
    const f = {};
    for (const [k, x] of Object.entries(v)) f[k] = { shape: shapeOf(x), optional: false };
    return { t: 'object', f };
  }
  return { t: typeof v };
}

function withNull(s) {
  return s.nullable ? s : { ...s, nullable: true };
}

function merge(a, b) {
  if (a.t === 'null') return b.t === 'null' ? a : withNull(b);
  if (b.t === 'null') return withNull(a);
  const nullable = a.nullable || b.nullable;
  let out;
  if (a.t === 'object' && b.t === 'object') {
    const f = {};
    for (const k of new Set([...Object.keys(a.f), ...Object.keys(b.f)])) {
      const x = a.f[k];
      const y = b.f[k];
      if (x && y) f[k] = { shape: merge(x.shape, y.shape), optional: x.optional || y.optional };
      else f[k] = { shape: (x ?? y).shape, optional: true };
    }
    out = { t: 'object', f };
  } else if (a.t === 'array' && b.t === 'array') {
    out = { t: 'array', of: a.of && b.of ? merge(a.of, b.of) : (a.of ?? b.of) };
  } else if (a.t === 'tuple' && b.t === 'tuple' && a.items.length === b.items.length) {
    out = { t: 'tuple', items: a.items.map((x, i) => merge(x, b.items[i])) };
  } else if (a.t === b.t) {
    out = { ...a };
  } else {
    const parts = [...(a.t === 'union' ? a.of : [a]), ...(b.t === 'union' ? b.of : [b])];
    const of = [];
    for (const p of parts) {
      const same = of.findIndex((q) => q.t === p.t && p.t !== 'union');
      if (same >= 0) of[same] = merge(of[same], p);
      else of.push({ ...p, nullable: false });
    }
    out = { t: 'union', of };
  }
  if (nullable) out.nullable = true;
  return out;
}

const ident = /^[A-Za-z_$][A-Za-z0-9_$]*$/;

function print(s, depth) {
  const pad = '  '.repeat(depth);
  let text;
  switch (s.t) {
    case 'null':
      // 시험 데이터에서 늘 비어 있던 칸: 꼴을 모른다.
      return 'unknown';
    case 'object': {
      const keys = Object.keys(s.f).sort();
      if (keys.length === 0) text = 'Record<string, unknown>';
      else {
        const lines = keys.map((k) => {
          const f = s.f[k];
          const shape = OPTIONAL.has(k) && f.shape.t !== 'null' ? withNull(f.shape) : f.shape;
          const type = k === 'tone' && shape.t === 'string' ? `Tone${shape.nullable ? ' | null' : ''}` : print(shape, depth + 1);
          return `${pad}  ${ident.test(k) ? k : JSON.stringify(k)}${f.optional ? '?' : ''}: ${type};`;
        });
        text = `{\n${lines.join('\n')}\n${pad}}`;
      }
      break;
    }
    case 'array':
      text = s.of ? `Array<${print(s.of, depth)}>` : 'unknown[]';
      break;
    case 'tuple':
      text = `[${s.items.map((i) => print(i, depth)).join(', ')}]`;
      break;
    case 'union':
      text = `(${s.of.map((i) => print(i, depth)).join(' | ')})`;
      break;
    default:
      text = s.t;
  }
  return s.nullable ? `${text} | null` : text;
}

const read = (p) => JSON.parse(readFileSync(join(fx, p), 'utf8'));

let snapshot = null;
let consts = null;
const extras = new Map();
for (const set of SETS) {
  const shared = read(`sets/${set}.json`);
  snapshot = snapshot ? merge(snapshot, shapeOf(shared.snapshot)) : shapeOf(shared.snapshot);
  consts = consts ? merge(consts, shapeOf(shared.consts)) : shapeOf(shared.consts);
  for (const page of read(`pages/${set}.json`)) {
    const extra = page.data.extra;
    if (extra === null) continue;
    const key = `${page.data.page}${extra.view ? `_${extra.view}` : ''}`;
    extras.set(key, extras.has(key) ? merge(extras.get(key), shapeOf(extra)) : shapeOf(extra));
  }
}

const camel = (s) => s.replace(/(^|_)([a-z])/g, (_, __, c) => c.toUpperCase());
let out = '// 만든 파일이다(tools/gen-types.mjs). 손으로 고치지 않는다: 시험 데이터를 다시 만들고 `npm run types`.\n';
out += '// 열거형 값은 string으로만 적힌다. 값의 뜻과 가능한 값은 docs/derived.md와 데몬의 타입을 본다.\n\n';
out += "import type { Tone } from './types';\n\n";
out += `/** 화면이 보는 데이터 전체(데몬의 \`admin::snapshot::Snapshot\`). */\nexport interface Snapshot ${print({ ...snapshot, nullable: false }, 0)}\n\n`;
out += `/** 화면이 쓰는 상수와 이름표(\`admin::derive::consts\`). */\nexport interface Consts ${print({ ...consts, nullable: false }, 0)}\n`;
for (const [key, shape] of [...extras].sort()) {
  out += `\n/** \`extra\`: ${key} */\nexport interface Extra${camel(key)} ${print({ ...shape, nullable: false }, 0)}\n`;
}
writeFileSync(join(root, 'src/lib/types.gen.ts'), out);
console.log(`types.gen.ts: Snapshot + Consts + ${extras.size} extras`);
