// 서버가 주는 답의 꼴. `Snapshot`과 화면별 덧붙임(`extra`)은 시험 데이터에서 만든 types.gen.ts에 있다.
import type { Consts, Snapshot } from './types.gen';

export type Tone = 'ok' | 'muted' | 'info' | 'warn' | 'bad';

export interface Flash {
  tone: Tone;
  text: string;
}

/** 홈의 "확인할 것" 한 줄. `page`는 가 볼 화면의 경로다. */
export interface Notice {
  tone: Tone;
  text: string;
  page: string;
}

export type PageKey =
  | 'overview'
  | 'work'
  | 'automation'
  | 'activity'
  | 'memory'
  | 'skills'
  | 'character'
  | 'expressions'
  | 'connections'
  | 'browser'
  | 'safety'
  | 'notify'
  | 'tasks'
  | 'system'
  | 'start';

/** 로그인한 화면 하나. */
export interface PageData<Extra = unknown> {
  kind: 'page';
  page: PageKey;
  /** 이 화면의 대표 경로(처리 뒤 돌아올 곳). 하위 화면이면 부모 화면의 경로다. */
  path: string;
  title: string;
  /** 관리 화면과 앱에 보이는 이름(결정 0064). 머리글, 맺음, 탭 제목이 쓴다. */
  name: string;
  csrf: string;
  flash: Flash | null;
  snapshot: Snapshot;
  /** 화면마다 다른 덧붙임(기억 항목, 스킬, 캐릭터 파일, 넘겨받기 상태 …). */
  extra: Extra;
  notices: Notice[];
  overall: { tone: Tone; label: string };
  /** 승인 대기 건수(메뉴의 배지). */
  pending: number;
  /** 데몬의 상수와 이름표. */
  consts: Consts;
}

/** 로그인 없이 여는 화면(데몬의 `admin::spa::Standalone`). */
export type Screen =
  | { type: 'login'; error: string | null; external: boolean }
  | { type: 'no_password' }
  | { type: 'setup'; token: string; error: string | null; min_chars: number }
  | { type: 'setup_invalid' }
  | { type: 'google_returned'; ok: boolean; text: string }
  | { type: 'site_login_form'; site: string; origin: string; token: string; minutes: number; attempts: number; error: string | null }
  | { type: 'site_login_busy'; site: string }
  | { type: 'site_login_done'; site: string; note: string }
  | { type: 'site_login_pending'; site: string; note: string; code_link: string | null }
  | { type: 'site_login_failed'; site: string; reason: string }
  | { type: 'site_login_invalid' }
  | { type: 'site_code_form'; site: string; origin: string; token: string; error: string | null }
  | { type: 'not_found' };

export interface StandaloneData {
  kind: 'standalone';
  /** 관리 화면과 앱에 보이는 이름(결정 0064). */
  name: string;
  screen: Screen;
}

export type ServerReply = PageData | StandaloneData | { redirect: string; flash?: Flash | null };
