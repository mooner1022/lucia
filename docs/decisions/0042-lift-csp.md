# 0042. 관리 페이지의 CSP 해제

- 날짜: 2026-11-18
- 상태: **결정 0061로 바뀌었다**(2026-11-21). 화면이 같은 출처의 스크립트 파일만 쓰는 정적 Svelte 빌드가 되어, 이 문서의 "되돌리려면"에 적은 대로 `script-src 'self'`로 좁힌 CSP를 다시 걸었다. 아래는 해제했을 때의 기록이다.
- 결정 방식: 사용자 지시 "csp를 해제해." (결정 0041의 보고에서 스크립트가 없어 못 하는 일이 있다고 알린 뒤)
- 대체: 결정 0002 "화면 정비"의 "CSP를 좁혔다"(`default-src 'none'; style-src 'self'; img-src 'self'; form-action 'self' https://accounts.google.com; frame-ancestors 'none'; base-uri 'none'`).

## 결정

- 관리 페이지 응답에서 `Content-Security-Policy` 머리를 뺐다(`admin::guard`). 이제 브라우저가 스크립트·인라인 스타일·바깥 출처 자원을 막지 않는다.
- **그대로 둔 것**: `X-Frame-Options: DENY`(다른 사이트가 이 페이지를 프레임에 넣는 것), `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `Cache-Control: no-store`, Host 확인(DNS 리바인딩), 로그인·CSRF·`SameSite=Strict` `HttpOnly` 쿠키, 일회용 토큰 링크, 출력 이스케이프(maud, `outside_text_is_escaped` 시험).
- 바뀐 시험: `security_headers_and_static_files`는 이제 CSP가 **없음**을 확인하고 나머지 머리를 확인한다. `pages_follow_the_csp`(스크립트·인라인 스타일 금지)는 지시를 거스르는 시험이라 `pages_link_the_stylesheet`로 줄였다.
- 화면은 아직 그대로다. 지금 마크업에는 스크립트도 인라인 스타일도 없고, 이 결정으로 화면이 달라지지는 않는다. 앞으로 스크립트를 쓸 수 있다는 것이 달라졌다.

## 잃은 것(알고 해제한 것)

- **주입 방어의 두 번째 겹**: 화면에 찍히는 바깥 글(메일 제목, 웹 검색·감시 결과, Discord 글, 활동 요약)에 이스케이프 구멍이 생기면, 예전에는 브라우저가 스크립트를 막아 피해를 줄였다. 지금은 이스케이프가 유일한 방어다. 이 페이지는 봇 토큰·관리 비밀번호를 다루고, LAN에서는 평문 HTTP다(결정 0002가 받아들인 절충).
- `form-action` 제한: 주입된 폼이 다른 곳으로 값을 보내는 것을 막던 것. Google 동의 화면으로의 이동 예외(`https://accounts.google.com`)도 필요 없어졌다.
- `base-uri`, `frame-ancestors`: 앞쪽은 `<base>` 주입을 막던 것, 뒤쪽은 `X-Frame-Options`가 이어서 맡는다.

## 되돌리려면

`admin::guard`의 머리 목록에 `header::CONTENT_SECURITY_POLICY`와 위 값(결정 0002 "화면 정비")을 다시 넣고, `security_headers_and_static_files`를 되돌린다. 스크립트를 더한 뒤라면 `script-src 'self'`로 좁히는 것이 현실적이다(인라인 스크립트 없이 `/static/`의 파일 하나).

## 아직 하지 않은 것

- 스크립트로 하는 기능(저장 전 변경 경고, 복사 버튼, Esc로 닫기, 수동 테마 바꾸기)은 이 결정으로 가능해졌을 뿐 만들지 않았다. 원하면 따로 지시해 달라.
