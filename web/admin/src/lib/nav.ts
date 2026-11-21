import type { PageKey } from './types';

export interface NavPage {
  key: PageKey;
  path: string;
  label: string;
  icon: string;
  /** 이 메뉴 칸이 켜지는 화면들(설정의 보조 탭은 한 칸을 나눠 쓴다). */
  also?: PageKey[];
}

/** 메뉴 묶음. 제목이 없는 묶음은 맨 위 한 줄이다. */
export const NAV: { title: string; pages: NavPage[] }[] = [
  { title: '', pages: [{ key: 'overview', path: '/', label: '홈', icon: 'home' }] },
  {
    title: '일',
    pages: [
      { key: 'work', path: '/work', label: '할 일', icon: 'tasks' },
      { key: 'automation', path: '/automation', label: '자동화', icon: 'repeat' },
      { key: 'activity', path: '/activity', label: '활동', icon: 'activity' },
    ],
  },
  {
    title: 'Lucia',
    pages: [
      { key: 'memory', path: '/memory', label: '기억', icon: 'memory' },
      { key: 'skills', path: '/skills', label: '스킬', icon: 'spark' },
      { key: 'character', path: '/character', label: '캐릭터', icon: 'chat' },
      { key: 'expressions', path: '/expressions', label: '이모지·스티커', icon: 'smile' },
    ],
  },
  {
    title: '연결',
    pages: [
      { key: 'connections', path: '/connections', label: '연결', icon: 'link' },
      { key: 'browser', path: '/browser', label: '브라우저', icon: 'globe' },
    ],
  },
  {
    title: '관리',
    pages: [
      { key: 'safety', path: '/settings', label: '설정', icon: 'sliders', also: ['notify', 'tasks'] },
      { key: 'system', path: '/system', label: '시스템', icon: 'server' },
      { key: 'start', path: '/start', label: '초기 설정', icon: 'flag' },
    ],
  },
];

/** 좁은 화면 아래 막대에 놓는 화면(나머지는 "메뉴"로 연다). */
export const QUICK: PageKey[] = ['overview', 'work', 'activity', 'memory'];

export const SETTINGS_TABS: { key: PageKey; href: string; label: string }[] = [
  { key: 'safety', href: '/settings', label: '안전과 한도' },
  { key: 'notify', href: '/settings/notify', label: '알림' },
  { key: 'tasks', href: '/settings/work', label: '작업과 보관' },
];

export function isCurrent(item: NavPage, current: PageKey): boolean {
  return item.key === current || (item.also?.includes(current) ?? false);
}
