// 개편 전에는 한 화면에 있었지만 새 짜임에서 **다른 화면으로 옮긴** 줄. 없애는 것이 아니다: 전체 목록 검사는 그대로 통과해야 한다.
// 옮길 때마다 까닭을 적는다. 비어 있는 것이 기본이다.

/** 화면 이름(개편 전 목록의 이름) → 그 화면에서 뺀 줄. */
export const MOVED: Record<string, string[]> = {};

/** `${원래 화면}\n${줄}` → 옮겨 간 화면 이름. */
export const MOVED_TO: Record<string, string> = {};
