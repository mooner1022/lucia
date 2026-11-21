import type { Tone } from '../lib/types';

/** 결마다의 아이콘. 색만으로 알리지 않게 배지·안내·줄 앞머리가 함께 쓴다. */
export const TONE_ICON: Record<Tone, string> = {
  ok: 'check-circle',
  bad: 'x-circle',
  warn: 'alert',
  info: 'info',
  muted: 'info',
};
