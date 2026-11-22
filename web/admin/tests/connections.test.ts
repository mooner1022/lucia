// 연결 화면의 앱 부분(결정 0066·0067): "Discord DM의 파일 받기" 범위 상자와 알림 받을 곳(앱 모두 또는 Discord DM).
// 개편 전 화면에 없던 기능이라 기능 보존 기준 목록(parity_baseline.txt)에는 없다. 새 기능의 보존은 여기서 지킨다.
// 알림 받을 곳에서 앱 하나를 고르던 선택지는 결정 0067에서 없앴다(기준 목록에서도 그 줄을 뺐다).
import { afterEach, describe, expect, it } from 'vitest';
import type { PageData } from '../src/lib/types';
import { clear, page, render } from './helpers';

afterEach(clear);

const BADGE = '알림 받는 곳';

/** 알림 받을 곳 고르기(`/app/notify`의 `client`)의 선택지 값과 지금 고른 값. */
function notifySelect(body: HTMLElement) {
  const select = body.querySelector<HTMLSelectElement>('form[action="/app/notify"] select[name="client"]');
  expect(select).not.toBeNull();
  return { values: [...select!.options].map((o) => o.value), labels: [...select!.options].map((o) => o.textContent?.trim()), value: select!.value };
}

/** 앱 하나를 더한 연결 화면 데이터(데몬이 계산해 주는 `ui` 값까지 손으로 채운다). 데몬은 `*`나 null만 준다(결정 0067). */
function withSecondApp(data: PageData, notifyClient: string | null): PageData {
  const next = structuredClone(data);
  const [first] = next.snapshot.clients[0];
  const second = structuredClone(first);
  second.id = 'second-app';
  second.name = 'second-app-name';
  next.snapshot.clients.push([second, true]);
  next.snapshot.app.notify_client = notifyClient;
  for (const [c] of next.snapshot.clients) {
    c.ui.is_notify_target = c.ui.is_app && c.revoked_at === null && notifyClient !== null;
  }
  return next;
}

describe('연결 화면: Discord DM의 파일 받기 범위', () => {
  it('발급 폼과 앱마다의 범위 폼에 상자가 있고, 범위가 있는 앱만 켜져 있다', () => {
    const p = page('full', '/connections');
    const body = render(p.data, p.url);
    const issue = body.querySelector('form[action="/clients/new"]');
    expect(issue?.querySelector('input[type="checkbox"][name="scope_owner_files"]')).not.toBeNull();
    expect(issue?.textContent).toContain('Discord DM의 파일 받기 허용');
    expect(issue?.textContent).toContain('기본은 꺼짐. Discord DM의 파일도 받습니다(다른 앱 대화의 파일은 늘 받습니다).');
    const forms = [...body.querySelectorAll('form')].filter((f) => /^\/clients\/[^/]+\/scopes$/.test(f.getAttribute('action') ?? ''));
    // 취소하지 않은 앱 하나(기기와 취소한 앱에는 범위 폼이 없다).
    expect(forms).toHaveLength(1);
    const box = forms[0].querySelector<HTMLInputElement>('input[type="checkbox"][name="scope_owner_files"]');
    expect(box).not.toBeNull();
    expect(box!.checked).toBe(false);
    expect(forms[0].textContent).toContain('Discord DM의 파일 받기 허용');
    expect(forms[0].textContent).toContain(
      'Discord DM에서 오간 파일도 이 앱에서 받습니다(다른 앱 대화의 파일은 늘 받습니다). 토큰이 새면 그 파일을 모두 내려받을 수 있습니다.',
    );
  });

  it('범위가 있으면 상자가 켜지고, 범위 이름이 목록에 적힌다', () => {
    const p = page('full', '/connections');
    const data = structuredClone(p.data);
    const [app] = data.snapshot.clients[0];
    app.scopes = ['owner_files'];
    app.ui.has_owner_files = true;
    app.ui.scope_labels = ['Discord DM의 파일 받기'];
    const body = render(data, p.url);
    const box = body.querySelector<HTMLInputElement>(`form[action="/clients/${app.id}/scopes"] input[name="scope_owner_files"]`);
    expect(box?.checked).toBe(true);
    expect(body.textContent).toContain('범위: Discord DM의 파일 받기');
  });
});

describe('연결 화면: 알림 받을 곳(앱 모두 또는 Discord DM)', () => {
  it('앱이 있으면 선택지는 Discord DM과 앱 모두 둘뿐이고, 앱 하나를 고르는 선택지는 없다', () => {
    for (const set of ['full', 'open', 'alt']) {
      const p = page(set, '/connections');
      const body = render(p.data, p.url);
      const { values, labels } = notifySelect(body);
      expect(values, set).toEqual(['', '*']);
      expect(labels, set).toEqual(['Discord DM', '앱 모두(사람이 있는 기기에서 울림)']);
    }
    // 앱이 둘이어도 마찬가지다(앱마다 한 줄씩 늘지 않는다).
    const full = page('full', '/connections');
    expect(notifySelect(render(withSecondApp(full.data, '*'), full.url)).values).toEqual(['', '*']);
  });

  it('앱이 없으면 앱 모두를 고르게 하지 않는다', () => {
    for (const set of ['fresh', 'healthy']) {
      const p = page(set, '/connections');
      const body = render(p.data, p.url);
      expect(notifySelect(body).values, set).toEqual(['']);
    }
  });

  it('데몬이 정한 값(앱으로 받으면 늘 *, Discord DM이면 비어 있음)으로 지금 고른 것이 보인다', () => {
    for (const set of ['open', 'full']) {
      const p = page(set, '/connections');
      expect(p.data.snapshot.app.notify_client, set).toBe('*');
      expect(notifySelect(render(p.data, p.url)).value, set).toBe('*');
    }
    const alt = page('alt', '/connections');
    expect(alt.data.snapshot.app.notify_client).toBeNull();
    expect(notifySelect(render(alt.data, alt.url)).value).toBe('');
  });

  it('도움말이 앱들이 한 대화를 나눈다고 알린다', () => {
    const p = page('full', '/connections');
    const form = render(p.data, p.url).querySelector('form[action="/app/notify"]');
    expect(form?.textContent).toContain(
      '앱 모두를 고르면 브리핑·알림·승인 카드가 모든 앱에 들어가고, 사람이 있는 기기에서 울립니다. 앱들은 기기가 여럿이어도 한 대화를 나눕니다. Discord DM에서 직접 한 대화의 답은 DM으로 옵니다.',
    );
  });

  it('"알림 받는 곳" 배지는 데몬의 표시(ui.is_notify_target)가 켜진 앱에만 붙는다', () => {
    const count = (body: HTMLElement) => (body.textContent?.match(new RegExp(BADGE, 'g')) ?? []).length;
    const full = page('full', '/connections');
    // 앱으로 받을 때: 취소하지 않은 앱 하나(기기와 취소한 앱은 아니다).
    expect(count(render(full.data, full.url))).toBe(1);
    // 앱을 하나 더하면 앱마다 붙는다.
    const all = withSecondApp(full.data, '*');
    expect(count(render(all, full.url))).toBe(2);
    // Discord DM: 없다.
    const dm = withSecondApp(full.data, null);
    expect(count(render(dm, full.url))).toBe(0);
  });
});

describe('초기 설정 화면: 알림 받을 곳(앱 모두)', () => {
  it('앱 단계의 선택지에 앱 모두가 있고, 앱이 없으면 없다', () => {
    const full = page('full', '/start');
    const body = render(full.data, full.url);
    // 앞의 둘은 연결 화면과 같다(Discord DM, 앱 모두). 앱 하나를 고르는 선택지는 결정 0067에서 없앴다.
    expect(notifySelect(body).values).toEqual(['', '*']);
    const open = page('open', '/start');
    expect(notifySelect(render(open.data, open.url)).value).toBe('*');
    const fresh = page('fresh', '/start');
    // 접속 API가 꺼져 있거나 앱이 없는 묶음에는 고르기 자체가 없다.
    const select = render(fresh.data, fresh.url).querySelector('form[action="/app/notify"] select[name="client"]');
    if (select !== null) expect([...(select as HTMLSelectElement).options].map((o) => o.value)).toEqual(['']);
  });

  it('앱 모두일 때도 배지가 붙는다', () => {
    const p = page('open', '/start');
    const body = render(p.data, p.url);
    expect(body.textContent).toContain(BADGE);
  });
});
