// 표기(시각, 숫자). 데몬의 `admin::snapshot`에 있던 것과 같은 글을 낸다(tests/format.test.ts가 데몬이 낸 값과 맞춘다).
// 시각은 브라우저의 시간대가 아니라 데몬이 정한 시간대(`snapshot.tz`)로 적는다.

const parts = new Map<string, Intl.DateTimeFormat>();

function local(at: Date, tz: string): { y: number; m: number; d: number; hh: string; mm: string; ss: string } {
  let f = parts.get(tz);
  if (!f) {
    f = new Intl.DateTimeFormat('en-CA', {
      timeZone: tz,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hourCycle: 'h23',
    });
    parts.set(tz, f);
  }
  const p: Record<string, string> = {};
  for (const x of f.formatToParts(at)) p[x.type] = x.value;
  return { y: Number(p.year), m: Number(p.month), d: Number(p.day), hh: p.hour, mm: p.minute, ss: p.second };
}

const pad = (n: number) => String(n).padStart(2, '0');

/** 사람이 읽는 시각: 가까우면 "3분 전", 오늘·어제·내일은 그 말로, 나머지는 날짜. */
export function when(at: string | Date, now: string | Date, tz: string): string {
  const a = new Date(at);
  const n = new Date(now);
  const seconds = Math.trunc((n.getTime() - a.getTime()) / 1000);
  const minutes = Math.trunc(seconds / 60);
  if (minutes === 0 && seconds >= 0) return '방금';
  if (minutes >= 1 && minutes < 60) return `${minutes}분 전`;
  if (minutes >= -59 && minutes < 0) return `${-minutes}분 뒤`;
  const l = local(a, tz);
  const t = local(n, tz);
  const days = Math.round((Date.UTC(l.y, l.m - 1, l.d) - Date.UTC(t.y, t.m - 1, t.d)) / 86_400_000);
  const hm = `${l.hh}:${l.mm}`;
  if (days === 0) return `오늘 ${hm}`;
  if (days === -1) return `어제 ${hm}`;
  if (days === 1) return `내일 ${hm}`;
  if (l.y === t.y) return `${pad(l.m)}월 ${pad(l.d)}일 ${hm}`;
  return `${l.y}-${pad(l.m)}-${pad(l.d)} ${hm}`;
}

/** 정확한 시각(마우스를 올리면 보인다). */
export function exact(at: string | Date, tz: string): string {
  const l = local(new Date(at), tz);
  return `${l.y}-${pad(l.m)}-${pad(l.d)} ${l.hh}:${l.mm}:${l.ss}`;
}

/** 천 단위 쉼표. */
export function num(n: number): string {
  return Math.trunc(n).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

export function usd(v: number): string {
  return v < 0.01 && v > 0 ? `$${v.toFixed(4)}` : `$${v.toFixed(2)}`;
}

export function bytes(n: number): string {
  return n < 1024 ? `${n} B` : `${(n / 1024).toFixed(1)} KB`;
}
