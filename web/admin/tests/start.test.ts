// 초기 설정 화면(결정 0064): 상태 묶음마다 알맞은 모습이 나오는지, 쓰기 동작의 폼이 모두 있는지, 홈의 안내가 알맞을 때만 보이는지 본다.
// 개편 전 화면이 아니라서 기능 보존 기준 목록(parity_baseline.txt)에는 없다. 새 기능의 보존은 여기서 지킨다.
import { afterEach, describe, expect, it } from 'vitest';
import { clear, inventory, page, pages, render } from './helpers';

afterEach(clear);

/** 한 상태 묶음의 초기 설정 화면을 그려 폼 목록과 글을 돌려준다. */
function draw(set: string) {
  const p = page(set, '/start');
  const body = render(p.data, p.url);
  const forms = new Set([...inventory(body)].filter((l) => l.startsWith('form post ')).map((l) => l.split(' ')[2]));
  // 소스의 줄바꿈과 들여쓰기는 한 칸으로 줄여 견준다.
  return { body, forms, text: (body.textContent ?? '').replace(/\s+/g, ' '), data: p.data };
}

const states = (data: ReturnType<typeof draw>['data']) => data.snapshot.setup.steps.map((s) => s.state);

describe('초기 설정 화면', () => {
  it('모든 상태 묶음에서 그려진다(여섯 단계, 단계마다 구역과 배지)', () => {
    for (const set of ['healthy', 'fresh', 'troubled', 'full', 'alt', 'open']) {
      const { body, data } = draw(set);
      expect(body.querySelector('h1')?.textContent, set).toBe('초기 설정');
      expect(body.textContent, set).not.toContain('이 화면을 그리지 못했습니다');
      const keys = data.snapshot.setup.steps.map((s) => s.key);
      expect(keys, set).toEqual(['profile', 'discord', 'google', 'app', 'images', 'env']);
      for (const step of data.snapshot.setup.steps) {
        const section = body.querySelector(`section#${step.key}`);
        expect(section, `${set}:${step.key}`).not.toBeNull();
        // 단계의 제목, 데몬이 계산한 한 줄 요약, 상태 배지가 구역에 있다.
        expect(section!.textContent, `${set}:${step.key}`).toContain(step.title);
        expect(section!.textContent, `${set}:${step.key}`).toContain(step.summary);
        expect(section!.querySelector('header')?.textContent, `${set}:${step.key}`).toContain(step.state_label);
      }
    }
  });

  it('구역은 단계의 순서대로 번호가 붙고, 건너뛰기 링크가 각 구역으로 간다', () => {
    const { body } = draw('full');
    const titles = [...body.querySelectorAll('main section > header h2')].map((h) => h.textContent?.trim());
    expect(titles).toEqual(['1. 이름과 모습', '2. Discord', '3. Google 메일·일정', '4. 앱과 푸시', '5. 이미지 생성', '6. 환경']);
    const jump = [...body.querySelectorAll('nav[aria-label="이 화면의 구역"] a')].map((a) => a.getAttribute('href'));
    expect(jump).toEqual(['#profile', '#discord', '#google', '#app', '#images', '#env']);
  });

  describe('막 시작한 설치(fresh)', () => {
    it('대화할 곳을 하나 연결하라고 하고, Discord와 앱이 해야 할 일이고 나머지는 선택이다', () => {
      const { text, forms, data } = draw('fresh');
      expect(data.snapshot.setup.complete).toBe(false);
      expect(text).toContain('대화할 곳을 하나 연결하세요.');
      expect(states(data)).toEqual(['optional', 'todo', 'optional', 'todo', 'optional', 'done']);
      // 안내를 끄는 단추는 있고 다시 켜는 단추는 없다.
      expect(forms.has('/start/dismiss')).toBe(true);
      expect(forms.has('/start/reopen')).toBe(false);
      // 이름·그림, Discord 둘, Google, 이미지 키.
      for (const action of [
        '/profile/name',
        '/profile/avatar',
        '/discord/token',
        '/discord/owner',
        '/google/client/upload',
        '/google/client',
        '/images/key',
      ]) {
        expect(forms.has(action), action).toBe(true);
      }
      // 프로필 그림이 없으니 지우기는 없고, 이미 있는 것을 바꾸는 폼도 없다.
      expect(forms.has('/profile/avatar/remove')).toBe(false);
      // 접속 API를 켰고 앱도 푸시도 아직 없으니 앱 단계는 토큰 발급, 알림 받을 곳, FCM 폼을 모두 보인다.
      for (const action of ['/clients/new', '/app/notify', '/app/fcm']) expect(forms.has(action), action).toBe(true);
      expect(text).not.toContain('접속 API가 꺼져 있습니다');
    });

    it('연결 화면과 같은 칸 이름과 한도로 보낸다', () => {
      const { body } = draw('fresh');
      const field = (action: string, name: string) => body.querySelector(`form[action="${action}"] [name="${name}"]`);
      expect(field('/discord/token', 'token')?.getAttribute('type')).toBe('password');
      expect(field('/discord/owner', 'user_id')?.getAttribute('inputmode')).toBe('numeric');
      expect(field('/google/client/upload', 'file')?.getAttribute('type')).toBe('file');
      expect(field('/profile/name', 'name')?.getAttribute('maxlength')).toBe('24');
      expect(field('/profile/avatar', 'file')?.getAttribute('accept')).toBe('image/png,image/jpeg');
      expect(body.querySelector('form[action="/profile/avatar"]')?.getAttribute('enctype')).toBe('multipart/form-data');
      expect(body.querySelector('form[action="/google/client/upload"]')?.getAttribute('enctype')).toBe('multipart/form-data');
    });

    it('이름 칸의 도움말은 무엇이 바뀌고 무엇이 그대로인지 분명히 말하고 캐릭터 화면으로 이어 준다', () => {
      const { body } = draw('fresh');
      const help = body.querySelector('form[action="/profile/name"] .help')?.textContent ?? '';
      expect(help).toBe(
        '관리 화면과 앱에 보이는 이름입니다. Lucia가 자기를 부르는 이름과 말투는 ‘캐릭터’에서 정하고, Discord에 보이는 이름은 Discord 개발자 포털에서 정합니다. 알림 글의 ‘Lucia’는 아직 그대로입니다.',
      );
      expect([...body.querySelectorAll('section#profile a')].map((a) => a.getAttribute('href'))).toContain('/character');
    });

    it('홈에 초기 설정 안내가 보이고 /start로 이어진다', () => {
      const home = page('fresh', '/');
      const body = render(home.data, home.url);
      expect(body.textContent).toContain('초기 설정이 남아 있습니다.');
      expect([...body.querySelectorAll('main a')].map((a) => a.getAttribute('href'))).toContain('/start');
    });
  });

  describe('끝났지만 안내를 끈 설치(troubled)', () => {
    it('끝나지 않았는데 안내를 껐으면 다시 켜는 단추가 있고, 홈에는 안내가 없다', () => {
      const { forms, data, text } = draw('troubled');
      expect(data.snapshot.setup.complete).toBe(false);
      expect(data.snapshot.setup.dismissed).toBe(true);
      expect(forms.has('/start/reopen')).toBe(true);
      expect(forms.has('/start/dismiss')).toBe(false);
      expect(text).toContain('홈 안내 다시 켜기');
      const home = page('troubled', '/');
      expect(render(home.data, home.url).textContent).not.toContain('초기 설정이 남아 있습니다.');
    });
  });

  describe('설정을 마친 설치(healthy, full)', () => {
    it('필수를 마쳤다고 하고 안내를 끄거나 켜는 단추는 없다. 홈에도 안내가 없다', () => {
      for (const set of ['healthy', 'full']) {
        const { forms, text, data } = draw(set);
        expect(data.snapshot.setup.complete, set).toBe(true);
        expect(text, set).toContain('필수 설정을 마쳤습니다.');
        expect(forms.has('/start/dismiss') || forms.has('/start/reopen'), set).toBe(false);
        const home = page(set, '/');
        const body = render(home.data, home.url);
        expect(body.textContent, set).not.toContain('초기 설정이 남아 있습니다.');
        // 사이드 메뉴에서는 늘 열 수 있다.
        expect([...body.querySelectorAll('nav[aria-label="주 메뉴"] a')].map((a) => a.getAttribute('href')), set).toContain('/start');
      }
    });

    it('접속 API가 켜져 있으면 앱 폼이 나오고, 푸시가 켜져 있으면 FCM 폼 대신 프로젝트가 보인다(full)', () => {
      const { forms, text, data } = draw('full');
      for (const action of ['/clients/new', '/app/notify']) expect(forms.has(action), action).toBe(true);
      expect(forms.has('/app/fcm')).toBe(false);
      expect(text).toContain(data.snapshot.app.fcm_project ?? data.snapshot.app.notify_client ?? '');
    });

    it('접속 API가 꺼져 있으면 앱 폼 대신 읽기 전용 안내와 설정 키가 나온다(healthy, alt)', () => {
      for (const set of ['healthy', 'alt']) {
        const { forms, text, data } = draw(set);
        expect(data.snapshot.gateway_binds, set).toEqual([]);
        expect(text, set).toContain('접속 API가 꺼져 있습니다');
        expect(text, set).toContain('[gateway] bind');
        expect(text, set).toContain('이 화면은 설정 파일을 고치지 않습니다');
        for (const action of ['/clients/new', '/app/notify', '/app/fcm']) expect(forms.has(action), `${set}:${action}`).toBe(false);
      }
    });
  });

  describe('프로필 그림이 있는 설치(full)', () => {
    it('현재 그림을 보이고 지우기를 묻는 창으로 둔다. 비밀 값은 없다', () => {
      const { body, forms, data } = draw('full');
      const avatar = data.snapshot.profile.avatar!;
      const img = body.querySelector('section#profile img');
      expect(img?.getAttribute('src')).toBe(`/profile/avatar/file/${avatar.id}`);
      expect(img?.getAttribute('alt')).toContain(data.name);
      expect(forms.has('/profile/avatar/remove')).toBe(true);
      expect(body.querySelector('section#profile')?.textContent).toContain(avatar.ui.facts);
      expect(body.querySelector('section#profile')?.textContent).toContain(avatar.ui.by_label);
      // 입력 칸에는 정한 이름이 들어 있다.
      expect((body.querySelector('form[action="/profile/name"] input[name="name"]') as HTMLInputElement).value).toBe(data.snapshot.profile.name);
    });

    it('이름을 정하지 않았으면 입력 칸은 비어 있고 기본 이름이 안내로 보인다', () => {
      const { body, data } = draw('healthy');
      const input = body.querySelector('form[action="/profile/name"] input[name="name"]') as HTMLInputElement;
      expect(input.value).toBe('');
      expect(input.getAttribute('placeholder')).toBe(data.consts.profile.default_name);
    });
  });

  describe('환경(읽기 전용)', () => {
    it('샌드박스와 점검은 상태와 터미널에서 돌릴 명령만 보인다(폼 없음)', () => {
      const { body, data } = draw('full');
      const section = body.querySelector('section#env')!;
      expect(section.querySelector('form')).toBeNull();
      for (const item of data.snapshot.setup.env) {
        expect(section.textContent).toContain(item.label);
        if (item.command) expect(section.textContent).toContain(item.command);
      }
      expect(section.textContent).toContain('luciad check');
      expect(section.textContent).toContain('deploy/create-sandbox.sh');
      expect([...section.querySelectorAll('a')].map((a) => a.getAttribute('href'))).toContain('/system#checks');
    });
  });

  it('바깥 글(봇 이름, 계정, 경로)은 마크업이 아니라 글로만 들어간다', () => {
    const p = page('full', '/start');
    const data = structuredClone(p.data);
    data.snapshot.setup.config_path = '<img src=x onerror=alert(1)>';
    data.snapshot.setup.env[0].detail = '<b>굵게</b>';
    data.snapshot.setup.steps[1].summary = '<script>alert(1)</script>';
    const body = render(data, p.url);
    expect(body.querySelector('img[onerror]')).toBeNull();
    expect(body.querySelector('section#env b')).toBeNull();
    expect(body.querySelector('script')).toBeNull();
    expect(body.textContent).toContain('<b>굵게</b>');
  });
});

describe('프로필 이름이 틀과 탭 제목에 실린다', () => {
  it('모든 화면의 머리글과 맺음이 데몬이 준 이름을 쓴다', () => {
    for (const p of pages('full')) {
      const body = render(p.data, p.url);
      const name = p.data.name;
      expect(name.length, p.name).toBeGreaterThan(0);
      expect(body.querySelector('.brand .name')?.textContent, p.name).toContain(name);
      expect(body.querySelector('.footer')?.textContent, p.name).toContain(`${name} · 관리 페이지`);
      expect(document.title.endsWith(` · ${name}`), `${p.name}: ${document.title}`).toBe(true);
    }
  });
});
