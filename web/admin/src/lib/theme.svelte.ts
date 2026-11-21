// 밝은·어두운 판 고르기. 기본은 기기 설정을 따른다. 고른 값은 이 브라우저에만 남는다(없어도 화면은 그대로 뜬다).
export type Theme = 'system' | 'light' | 'dark';

const KEY = 'lucia-admin-theme';

function read(): Theme {
  try {
    const v = localStorage.getItem(KEY);
    return v === 'light' || v === 'dark' ? v : 'system';
  } catch {
    return 'system';
  }
}

export const theme = $state({ value: 'system' as Theme });

function applyTheme(value: Theme) {
  if (value === 'system') delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = value;
}

export function initTheme() {
  theme.value = read();
  applyTheme(theme.value);
}

export function cycleTheme() {
  const next: Theme = theme.value === 'system' ? 'light' : theme.value === 'light' ? 'dark' : 'system';
  theme.value = next;
  applyTheme(next);
  try {
    if (next === 'system') localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, next);
  } catch {
    // 저장하지 못해도 이번 화면에는 적용된다.
  }
}

export const THEME_LABEL: Record<Theme, string> = { system: '기기 설정', light: '밝게', dark: '어둡게' };
export const THEME_ICON: Record<Theme, string> = { system: 'monitor', light: 'sun', dark: 'moon' };
