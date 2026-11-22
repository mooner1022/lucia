# Lucia 앱 접속 API 사양서(프로토콜 판 2)

- 대상: Lucia 전용 앱(Android·iOS·웹·데스크톱)을 만드는 개발자
- 기준 구현: `crates/luciad/src/gateway.rs`, `gateway/methods.rs`, `gateway/http.rs`, `gateway/handover.rs`, `gateway/alerts.rs`, `gateway/notes.rs`, `gateway/tasks.rs`, `apppush.rs`. 이 문서와 코드가 다르면 코드가 맞고, 이 문서를 고친다.
- 설계 까닭: 결정 0021(판 1), 결정 0043(판 2, 외부 노출, 푸시). 앱에서 로그인 정보 넣기와 브라우저 화면 넘겨받기는 결정 0063. 데스크톱 앱이 낸 확장은 결정 0066. 앱이 기기가 여럿이어도 한 대화로 보이는 것은 결정 0067.
- 판: `2`(`welcome.v`). 하위 호환이 깨지는 변경이 있으면 판을 올린다.
- 이 사양서를 낸 뒤 바뀐 것만 모은 문서는 `docs/app-api-changes.md`다(새 변경이 위에 쌓인다). 결정 0066·0067의 변경은 그 문서의 2026-11-22 항목들에 있고, 이 사양서의 본문에도 합쳐 놓았다.
- 데스크톱 앱이 낸 확장(결정 0066)은 모두 같은 판 안에서 필드·프레임·메서드가 늘어난 것이다. 앱은 `welcome.capabilities`·`methods`·`scopes`의 표시를 보고 쓴다. 더해진 것:
  - 범위 `owner_files`(3.4), `welcome.capabilities`의 새 표시(5.3)
  - 앱 → 데몬 프레임 `desk`, `seen`, `presence`의 `away`, `message.context`, `result.files`(5.6, 7.1)
  - 데몬 → 앱 프레임 `seen`, `alert`, `notes`, `tasks`, 번호 붙는 프레임의 `alert: false`(5.4, 5.5)
  - 메서드 `notes.list`·`notes.put`·`notes.remove`, `tasks.list`(6.8, 6.9), `history`의 `shape: 2`(6.2)
  - `hello.tools[]`의 `when`·`timeout`, `invoke.timeout`(7.1, 7.2)
  - `POST /v1/files?keep=short`(8.4), `POST /v1/voice`(8.10, 하지 않음), 푸시 `clear`(10.4)
  - 알림 받을 곳 "앱 모두"와 사람이 있는 기기로 넘기기(11.1)
- 앱은 기기가 여럿이어도 한 대화다(결정 0067). 앱 대화의 프레임은 모든 앱의 상자에 들어가고, 한 앱에서 한 말은 다른 앱에 `sent`로 간다. 앞의 확장과 같은 판 안에서 더해진 것이다:
  - 데몬 → 앱 프레임 `sent`(5.5), `welcome.capabilities.shared_conversation`(5.3), 다른 앱 대화의 프레임에 붙는 `conversation`(5.5)
  - `history`의 `scope: "app"`이 모든 앱 대화(6.2), 다른 앱 대화의 파일과 카드(8.5, 5.6)
  - 알림 받을 곳은 "앱 모두"와 Discord DM 둘(11장), 울릴 곳 고르기에 "물은 앱 먼저"(11.1), 채팅 오류는 말이 온 곳으로(11장)
  - 앱이 할 일은 12.8
- 커스텀 이모지·스티커 확장은 `docs/app-expressions.md`에 있다(데몬 구현됨, 결정 0045). 이 문서에 더해지는 것:
  - `welcome`과 `GET /v1/me`의 `expressions` 필드, 번호 없는 `expressions` 프레임, 번호 붙는 `sticker` 프레임
  - `message.sticker`, 오류 `bad_sticker`
  - `GET /v1/expressions`, `GET /v1/expressions/files/{file_id}`
  - 푸시 `data.kind`의 `sticker`

---

## 목차

1. [개요](#1-개요)
2. [접속 정보](#2-접속-정보)
3. [인증, 토큰, 범위](#3-인증-토큰-범위)
4. [공통 규칙](#4-공통-규칙)
5. [WebSocket 프로토콜](#5-websocket-프로토콜)
6. [메서드(`call`)](#6-메서드call)
7. [데몬의 부탁(`invoke`)](#7-데몬의-부탁invoke)
8. [HTTP API](#8-http-api)
9. [데이터 형식](#9-데이터-형식)
10. [푸시(FCM)](#10-푸시fcm)
11. [메시지가 가는 곳(라우팅)](#11-메시지가-가는-곳라우팅)
12. [앱 구현 지침](#12-앱-구현-지침)
13. [보안 요구 사항](#13-보안-요구-사항)
14. [한도와 시간 모음](#14-한도와-시간-모음)
15. [오류 코드 모음](#15-오류-코드-모음)
16. [기기 역할(`device`)](#16-기기-역할device)
17. [예시 흐름](#17-예시-흐름)
18. [판 1에서 바뀐 점](#18-판-1에서-바뀐-점)

---

## 1. 개요

Lucia는 홈서버에서 도는 1인용 비서다. 데몬 `luciad`가 대화, 기억, 일정, 승인을 맡는다. 앱은 Discord와 같은 자리의 **채널**이다.

- 앱에서 한 말은 Discord에서 한 말과 같은 Lucia, 같은 기억, 같은 상주 세션으로 간다.
- Lucia의 답, 진행 안내, 파일, 승인 카드가 앱으로 온다. 앱은 카드의 버튼을 눌러 승인하거나 거절한다.
- 통신 길은 둘이다.
  - **WebSocket**(`/v1/ws`): 실시간 대화, 카드, 메서드 호출.
  - **HTTP**(`/v1/...`): 파일 올리기·받기, 연결 없이 하는 일(기록 읽기, 푸시 토큰 등록).
- 앱이 연결돼 있지 않을 때 보낸 것은 서버의 **보낸 프레임 상자**에 번호(`seq`)와 함께 남는다. 앱은 다시 연결할 때 받는다. 푸시를 설정했으면 FCM으로 앱을 깨운다.

---

## 2. 접속 정보

### 2.1 주소

| 구분 | WebSocket | HTTP 기준 주소 |
|---|---|---|
| 집 안(LAN·VPN) | `ws://<LAN 주소>:<포트>/v1/ws` | `http://<LAN 주소>:<포트>` |
| 밖(공개 주소) | `wss://<공개 호스트>/v1/ws` | `https://<공개 호스트>` |

- `<LAN 주소>:<포트>`는 데몬 설정 `[gateway] bind`의 값이다. 예: `192.168.0.10:8790`.
- 공개 호스트는 설정 `[public] gateway_url`의 값이다. TLS는 리버스 프록시(Traefik)가 맡는다.
- 관리 페이지에는 쓸 수 있는 주소가 보인다(연결 → "앱·기기 연결"). 앱은 주소를 사용자가 넣게 하거나, 집 안과 밖 둘을 저장해 두고 고른다.

### 2.2 받는 출처

| 연결이 들어온 길 | 받는가 |
|---|---|
| 루프백, 사설(10/8, 172.16/12, 192.168/16), 링크 로컬, CGNAT(100.64/10, Tailscale 등), IPv6 ULA·링크 로컬에서 직접 | 받는다 |
| 데몬이 믿는 리버스 프록시를 거쳐(공개 주소가 설정됐을 때) | 받는다 |
| 데몬이 믿는 리버스 프록시를 거쳐(공개 주소가 설정되지 않았을 때) | `403` |
| 그 밖의 주소에서 직접 | `403` |
| 믿는 프록시가 `X-Forwarded-For`를 주지 않았다 | `400` |
| 믿지 않는 상대가 `X-Forwarded-For`나 `Forwarded` 머리를 실었다 | `400` |

> **앱은 `X-Forwarded-For`·`Forwarded` 머리를 보내지 않는다.** 집 안에서 직접 붙을 때 이 머리가 있으면 거절된다.

- WebSocket 업그레이드가 출처 때문에 거절되면 본문 없이 HTTP 상태만 온다.
- HTTP API는 같은 경우 `{"error": {"code": "forbidden", ...}}`를 돌려준다(상태는 `403`이나 `400`).

### 2.3 TLS

- 밖에서는 반드시 `wss`/`https`로 붙는다. 인증서는 공인 CA(Let's Encrypt)의 것이다.
- 집 안 주소는 평문(`ws`/`http`)이다. 집 LAN이나 VPN을 믿는다는 전제다.
  - Android는 평문을 쓰려면 Network Security Config에서 그 주소를 허용해야 한다.
  - iOS는 ATS 예외가 필요하다.
  - 밖에서도 VPN(Tailscale·WireGuard)으로 집 안 주소에 붙을 수 있다.

---

## 3. 인증, 토큰, 범위

### 3.1 토큰

- 관리 페이지 → 연결 → "앱·기기 연결"에서 이름을 붙여 발급한다. 역할은 `app`이다.
- 형식: `lucia_` 다음에 소문자 16진수 64자. 약 244비트이고 길이는 70자다.
- **발급 직후 한 번만 보인다.** 서버에는 SHA-256 해시만 남는다. 앱은 기기의 안전한 저장소(Android Keystore 기반 암호화 저장소, iOS Keychain)에 둔다.
- 사용자가 관리 페이지에서 취소할 수 있다. 취소되면:
  - 열려 있던 WebSocket은 다음 프레임을 받을 때 `unauthorized` 오류를 보내고 끊는다. 관리 페이지에서 취소하면 바로 끊기기도 한다.
  - HTTP는 `401 unauthorized`를 돌려준다.
  - 그 클라이언트의 보낸 프레임 상자와 푸시 토큰은 지워진다.
- 클라이언트는 20개까지 발급된다.

### 3.2 인증 방법

| 길 | 방법 |
|---|---|
| WebSocket | 연결 후 첫 프레임 `hello{token}`(10초 안). 업그레이드 요청의 머리는 보지 않는다. |
| HTTP | `Authorization: Bearer <토큰>`. `Bearer `(대문자 B, 띄어쓰기 하나)로 시작해야 한다. |

`/v1/health` 말고 모든 HTTP 요청에 인증이 필요하다. 기기 역할 토큰은 HTTP API를 쓸 수 없다(`403 forbidden`).

### 3.3 인증 실패 막기

- 한 주소(실제 상대 주소)에서 인증 실패가 쌓이면 막는다. WebSocket과 HTTP가 같은 셈을 쓴다.
  - 첫 실패로부터 10분 안에 10번이면, 그 10분이 끝날 때까지 맞는 토큰도 거절한다.
  - 거절은 WebSocket `too_many_attempts`, HTTP `429`다.
- 실패로 세는 것은 토큰이 틀린 경우뿐이다. 다시 연결하는 것 자체는 세지 않는다.

### 3.4 범위(scopes)

기본 권한 밖의 일은 범위로만 할 수 있다. 사용자가 관리 페이지에서 토큰마다 켜고 끈다.

| 범위 | 허용하는 일 | 기본 |
|---|---|---|
| (없음, 기본) | 대화, 카드 누르기(입력 칸이 있는 카드 포함, 9.1), 기록 읽기, 파일, 푸시 등록, 이벤트, 권한 모드 **보기**, 주 모델 보기·바꾸기, 대화 정리(`session.compact`), 메모(`notes.*`, 6.8), 맡은 일 보기(`tasks.list`, 6.9), 책상 상태 알리기(`desk`, 5.6) | 늘 |
| `security_mode` | 권한 모드 **바꾸기**(`command` 메서드의 `option`) | 꺼짐 |
| `app_tools` | Lucia가 이 앱이 내준 기능(`hello.tools`, 7.2)을 부른다 | 꺼짐 |
| `browser_handover` | 브라우저 화면 넘겨받기(6.7, 8.9): 로그인된 브라우저의 화면을 보고 조작한다 | 꺼짐 |
| `owner_files` | Discord 알림 대화(DM)의 파일 받기(8.5). 기록(`scope: "owner"`)에 보인 DM의 파일을 내려받는다. 다른 앱 대화의 파일은 이 범위 없이 받는다(결정 0067) | 꺼짐 |

- 클라이언트의 범위는 `welcome.scopes`와 `GET /v1/me`의 `scopes`에 있다.
- 범위가 필요한 메서드는 그 범위가 없으면 `welcome.methods`에 보이지 않고, 불러도 `forbidden`이다. HTTP `GET /v1/browser/handover/frame/{tab}`(8.9)도 `browser_handover`가 없으면 `403 forbidden`이다.
- `browser_handover`는 토큰이 새면 **로그인된 브라우저 세션을 보고 조작할 수 있게 되는** 범위다(13장). 관리 페이지 → 연결에서 토큰마다 "브라우저 화면 넘겨받기 허용"으로 켠다.
- `owner_files`는 토큰이 새면 **Discord DM에서 받은 사진·문서를 모두 내려받을 수 있게 되는** 범위다(13장). 기록을 `owner`로 읽는 토큰은 이미 글을 다 보지만, 파일은 무게가 다르다. 관리 페이지 → 연결에서 토큰마다 "Discord DM의 파일 받기 허용"으로 켠다. 범위가 없는 토큰은 기록에 보인 DM의 파일에 `reachable: false`가 붙는다(6.2). 앱 대화는 하나라서 다른 앱 대화의 파일은 이 범위가 없어도 받는다(다른 앱이 짧게 둔 파일은 빼고, 8.5).
- 범위는 매 프레임마다 다시 읽는다. 바꾸면 다음 프레임부터 듣는다.

---

## 4. 공통 규칙

- **인코딩**: UTF-8 JSON. WebSocket은 글(Text) 프레임만 쓴다. 바이너리 프레임은 `bad_frame` 오류를 받고 무시된다(연결은 그대로).
- **글자 수**: "자"는 유니코드 스칼라 값(코드 포인트) 수다. 한글 한 글자는 1자다.
- **시각**: RFC 3339 UTC(예: `2026-11-19T03:13:06.534831+00:00`).
- **ID 모양**: 앱은 아래 ID를 해석하지 말고 그대로 쓴다.
  - 클라이언트: `cl-` + 16진수 10자
  - 대화: `app:<클라이언트 ID>`. 대화 ID는 앱마다 다르지만 **모든 앱이 하나의 타임라인을 본다**(결정 0067, 11장). 이 ID는 "어느 앱에서 한 말인가"를 가를 뿐이다. 알림 받을 곳을 "앱 모두"로 두면 그 알림의 대화는 `app:*`다(11.1).
  - 카드 참조: `app:<클라이언트 ID>:<16진수 12자>`. 앱 모두의 카드는 `app:*:<16진수 12자>`다.
  - 파일: `f_` + 16진수 12자
  - 데몬의 부탁: `d-` + 16진수 12자
  - 맡은 일: `g-`(목표)·`t-`(작업)·`f-`(후속 조치) + 저장소의 ID(6.9)
  - 메모: `n-` + 영문 소문자·숫자 6~16자. 만드는 쪽이 정한다(6.8).
- **넓혀질 수 있음**:
  - 앱은 **모르는 필드를 무시한다.**
  - 데몬 → 앱의 **모르는 `type`도 무시한다**(오류로 다루지 않는다).
  - 같은 판 안에서는 필드와 프레임 종류가 늘어날 수만 있다.
- **글 서식**: Lucia의 글(`text.body`, `note.body`, 카드의 `body`·`fields[].value`)은 Discord식 마크다운 하위 집합이다.
  - 굵게 `**`, 기울임 `*`, 코드 `` ` ``/```` ``` ````, 목록, 링크 `<URL>`·`[글](URL)`, 인용 `>`.
  - 모델이 쓴 글이 카드에 들어갈 때는 마크다운 기호가 `\`로 이스케이프돼 있다.
  - 앱은 마크다운을 그리되, **HTML로 해석하지 않는다.** 링크는 누를 때 사용자 확인을 거쳐 연다.

---

## 5. WebSocket 프로토콜

### 5.1 연결 수명

```
연결 ──hello──▶ welcome ──(resume을 줬으면) 다시 받는 프레임들 → resumed──▶ 주고받기 ──▶ 끊김
   └─ 10초 안에 hello가 없으면 아무 말 없이 끊는다
   └─ hello 실패: error 프레임 하나를 보내고 끊는다
```

- 같은 토큰으로 새로 연결하면 예전 연결은 데몬 쪽에서 닫힌다. 마지막 연결 하나만 산다.
- 데몬이 멈추거나 다시 시작하면 연결이 끊긴다. 앱은 다시 연결한다(12.3).

### 5.2 `hello`(앱 → 데몬, 첫 프레임)

```json
{"type": "hello", "token": "lucia_…", "resume": 41, "features": ["location"],
 "tools": [{"name": "location.get", "description": "휴대폰의 현재 위치", "params": {"precise": "boolean, 정밀 위치"}}]}
```

| 필드 | 형식 | 필수 | 설명 |
|---|---|---|---|
| `token` | 문자열 | 예 | 3.1의 토큰 |
| `resume` | 정수 | 아니오 | 앱이 마지막으로 **처리한** 프레임 번호다. 주면 그 뒤의 프레임을 다시 받는다(5.4). 처음이면 `0`을 준다(남아 있는 것 전부). 빼면 다시 받지 않는다. |
| `features` | 문자열 배열 | 아니오 | 데몬이 부탁(`invoke`, 7장)할 수 있는 앱 기능 이름. 정리 규칙은 아래와 같다. |
| `tools` | 객체 배열 | 아니오 | **Lucia에게 내주는 기능**(7.2). 여기 적은 것만 Lucia가 보고 부를 수 있다. |

`features` 정리 규칙:
- 앞뒤 공백을 지우고 소문자로 바꾼다.
- `[a-z0-9_-]`만 쓸 수 있고 40자까지다. 어기는 이름은 버린다.
- 중복을 지우고 정렬한 뒤 32개까지 남긴다.
- 정리된 결과는 `welcome.features`로 돌아온다.

`hello` 오류:

| 경우 | 오류 `code` | 그 뒤 |
|---|---|---|
| 첫 프레임이 `hello`가 아니다 | `hello_required` | 끊음 |
| 토큰이 틀렸거나 취소됐다 | `unauthorized` | 끊음(실패로 센다) |
| 그 주소가 막혀 있다 | `too_many_attempts` | 끊음 |
| 받지 않는 역할 | `unsupported_role` | 끊음 |
| 10초 안에 아무것도 없다 | (프레임 없음) | 끊음 |

### 5.3 `welcome`(데몬 → 앱)

```json
{
  "type": "welcome",
  "v": 2,
  "client": "cl-3f9a0c12de",
  "name": "휴대폰 앱",
  "role": "app",
  "conversation": "app:cl-3f9a0c12de",
  "seq": 57,
  "scopes": [],
  "methods": ["command", "commands", "history", "notes.list", "notes.put", "notes.remove",
              "push.register", "push.unregister", "session.compact", "tasks.list"],
  "features": ["location"],
  "tools": ["location.get"],
  "notify_target": true,
  "push": {"available": true, "registered": false},
  "profile": {"name": "Lucia",
              "avatar": {"id": "a_cfc0dacd5dcd71abf496c418", "mime": "image/png", "width": 800, "height": 800,
                         "size": 579183, "updated_at": "2026-11-20T04:43:07Z"}},
  "capabilities": {
    "max_message_chars": 8000,
    "buttons": true,
    "edit": true,
    "prompts": true,
    "forms": true,
    "attachments": true,
    "max_upload_bytes": 31457280,
    "max_message_files": 10,
    "history_shape": 2,
    "owner_files": true,
    "tool_when": true,
    "tool_files": true,
    "short_files": true,
    "tool_timeout": true,
    "desk": true,
    "presence_away": true,
    "message_context": true,
    "seen": true,
    "shared_conversation": true
  }
}
```

| 필드 | 형식 | 설명 |
|---|---|---|
| `v` | 정수 | 프로토콜 판(2). 앱이 아는 판보다 크면 사용자에게 앱 업데이트를 권한다. |
| `client` | 문자열 | 이 클라이언트 ID |
| `name` | 문자열 | 발급할 때 붙인 이름 |
| `role` | 문자열 | `app` |
| `conversation` | 문자열 | 이 앱의 대화 ID. 대화는 앱마다 따로 ID를 갖지만 타임라인은 하나다(4장, 11장). |
| `seq` | 정수 | 이 클라이언트에 지금까지 매긴 마지막 프레임 번호(없으면 0) |
| `scopes` | 문자열 배열 | 3.4 |
| `methods` | 문자열 배열 | 이 토큰이 부를 수 있는 메서드(6장), 정렬됨. 브라우저를 켠 데몬이고 토큰에 `browser_handover` 범위가 있으면 `browser.handover.state`, `.start`, `.input`, `.end`가 더해진다(6.7). 메모(`notes.list`, `notes.put`, `notes.remove`, 6.8)와 맡은 일(`tasks.list`, 6.9)은 범위 없이 늘 있다. `notes.list`·`tasks.list`가 있는지로 그 기능이 있는 데몬인지 안다. |
| `features` | 문자열 배열 | 정리된 `hello.features`(`tools`의 첫 마디가 더해진다) |
| `tools` | 문자열 배열 | 받아들인 `hello.tools`의 이름(7.2). Lucia가 실제로 부르려면 `scopes`에 `app_tools`가 있어야 한다. |
| `notify_target` | 불 | 알림(브리핑·알림·승인 카드)을 앱으로 받는가(11장). 알림 받을 곳이 "앱 모두"이면 모든 앱에 `true`이고, Discord DM이면 모든 앱에 `false`다. |
| `push.available` | 불 | 데몬에 FCM 자격 증명이 있어 푸시를 보낼 수 있다 |
| `push.registered` | 불 | 이 앱의 푸시 토큰이 등록돼 있다 |
| `profile` | 객체 | Lucia의 프로필(8.7): 사용자가 정한 `name`과 `avatar`. 이 필드가 없으면 프로필 기능이 없는 데몬이다. `profile.avatar`가 `null`이면 정한 그림이 없다(앱의 기본 모습을 쓴다). `profile.name`은 사용자가 관리 페이지에서 정한 이름이고, 정하지 않았으면 `"Lucia"`다(`name`이 없는 데몬은 기능이 붙기 전이니 앱이 자기 기본 이름을 쓴다). |
| `capabilities.attachments` | 불 | 파일 올리기를 받는다 |
| `capabilities.max_upload_bytes` | 정수 | 파일 하나 상한(바이트) |
| `capabilities.max_message_files` | 정수 | 말 하나에 붙이는 파일 수 상한 |
| `capabilities.buttons`, `edit`, `prompts` | 불 | 카드 버튼, 카드 고쳐 그리기(`card_closed`), 값을 받는 버튼을 쓴다(늘 `true`) |
| `capabilities.forms` | 불 | 여러 칸을 받는 버튼(`form`, 9.1)을 쓴다(늘 `true`). 비밀번호 칸이 들어 있을 수 있다. 이 필드가 없으면 `form`을 모르는 데몬이다. |
| `capabilities.history_shape` | 정수 | `2`: `history`의 `shape: 2`(`parts`, `files`)를 안다(6.2) |
| `capabilities.owner_files` | 불 | 범위 `owner_files`(3.4)를 아는 데몬이다. **이 토큰에 범위가 있다는 뜻이 아니다**(그것은 `scopes`로 본다). |
| `capabilities.tool_when` | 불 | `hello.tools[].when`을 지킨다(7.2) |
| `capabilities.tool_files` | 불 | `result.files`를 받는다(7.1) |
| `capabilities.short_files` | 불 | `POST /v1/files?keep=short`을 받는다(8.4). 파일을 받지 않는 데몬이면 `false`다. |
| `capabilities.tool_timeout` | 불 | `hello.tools[].timeout`을 지키고 `invoke`에 `timeout`을 싣는다(7.1, 7.2) |
| `capabilities.desk` | 불 | `desk` 프레임을 받는다(5.6) |
| `capabilities.presence_away` | 불 | `presence`의 `away`를 받는다(5.6) |
| `capabilities.message_context` | 불 | `message.context`를 받는다(5.6) |
| `capabilities.seen` | 불 | `seen`·`alert` 프레임을 주고받는다(5.5, 5.6, 11.1) |
| `capabilities.shared_conversation` | 불 | 앱이 기기가 여럿이어도 한 대화다(결정 0067): 다른 앱에서 한 말이 `sent`(5.5)로 오고, `history`의 `scope: "app"`이 모든 앱 대화(6.2)다. 이 필드가 없으면 앱마다 따로 대화하는 예전 데몬이다. |

- 위 표시(`history_shape`부터 `seen`까지)는 모두 데스크톱 앱이 낸 확장(결정 0066)이다. **앱은 표시가 있을 때만 새 프레임·필드를 보낸다.** 표시가 없는 예전 데몬은 모르는 `type`이나 `presence.state`에 `bad_frame`으로 답한다.
- `capabilities.voice`는 없다. 이 데몬에는 받아쓰기가 없다(8.10).
- 데몬 → 앱의 새 필드와 프레임(`alert: false`, `seen`, `alert`, `notes`, `tasks`, `sent`, 다른 앱 대화의 프레임에 붙는 `conversation`)은 표시 없이 온다. 모르는 것은 무시하면 된다(4장). `shared_conversation`은 그것이 오는 데몬인지 알려 줄 뿐이다(12.8).

### 5.4 번호, 받음 확인, 다시 받기

아래 **번호 붙는 프레임**에는 `seq`가 붙고 서버 상자에 남는다: `text`, `note`, `files`, `card`, `card_closed`, `sent`(그리고 `docs/app-expressions.md`의 `sticker`).

- `seq`는 클라이언트마다 1부터 1씩 늘어난다. 지워져도 다시 쓰지 않는다. **번호는 앱마다 제 것이다**: 앱 대화의 같은 프레임이 모든 앱의 상자에 들어가지만(11장) 번호는 앱마다 따로 매긴다(기기마다 커서). 앱이 모르는 `type`의 프레임도 번호가 있으면 `lastSeq`를 올리고 `ack`한다(건너뛴 번호를 잃은 것으로 보지 않는다).
- 앱은 프레임을 **처리한 뒤** `{"type": "ack", "seq": N}`을 보낸다.
  - 누적이다: N과 그 앞이 모두 처리됐다는 뜻이다.
  - 모아서 보내도 된다(12.2). 줄어드는 값은 무시된다. 답은 없다.
  - `ack`는 "처리했다"는 뜻이고 데몬이 상자를 비우는 데 쓴다. 사용자가 **봤다**는 `seen`(5.6)과 다르다.
- **`alert: false`**: `text`·`files`·`card`(와 `sticker`) 프레임에 `"alert": false`가 붙을 수 있다. 이 프레임은 화면에 놓되 말풍선·운영체제 알림·소리·깜박임을 내지 않는다. **필드가 없으면 울린다**(예전과 같다).
  - 앱 대화로 가는 이 프레임은 모두 이 규칙을 탄다(결정 0067). 앱이 여럿이면 데몬이 울릴 곳을 고르고(11.1), 울리지 않을 앱의 상자에만 붙는다. 다른 앱에서 한 말의 답과 그 대화의 카드도 같다.
  - 데몬이 나중에 `alert{seq}` 프레임을 보내면 그 프레임을 지금 알린다(5.5).
  - 다시 받기(`replay: true`)로 와도 `alert: false`는 그대로다.
  - `note`, `card_closed`, `sent`에는 붙지 않는다(원래 울리지 않는다).
- 다시 받기: `hello.resume = R`이면 `seq > R`인 프레임을 순서대로 보낸다.
  - 최대 500개이고, 각 프레임에 `"replay": true`가 붙는다.
  - 끝에 `resumed`가 온다.

```json
{"type": "resumed", "from": 41, "to": 57, "last": 57, "truncated": false}
```

| 필드 | 설명 |
|---|---|
| `from` | 앱이 준 `resume` |
| `to` | 이번에 다시 보낸 마지막 `seq`(없으면 `from`) |
| `last` | 지금까지 매긴 마지막 번호 |
| `truncated` | `true`이면 다시 받을 수 없는 구간이 있었다. 보관 기간이 지났거나 500개를 넘었다. 앱은 `history`로 기록을 다시 읽고, `seq`는 `last`로 맞춘다. 500개를 넘은 경우는 `resume = to`로 다시 연결해 나머지를 받아도 된다. |

- **중복**: 다시 받는 프레임과 그 사이 새로 생긴 프레임이 겹칠 수 있다. **앱은 이미 처리한 `seq` 이하의 프레임을 버린다.**
- **보관**: 받았다고 알린(ack) 프레임은 7일, 받았다고 알리지 않은 프레임도 30일 뒤 지운다. 한 시간마다 정리한다.
- `resume`을 주지 않으면 끊긴 동안의 프레임은 오지 않는다. 앱은 `seq`를 하나라도 받았으면 늘 `resume`을 준다.

### 5.5 데몬 → 앱 프레임

#### 번호 붙는 프레임

**`text`**: Lucia의 말.

```json
{"type": "text", "seq": 58, "body": "오늘 3시에 회의가 있어요."}
```

- `body`는 8000자까지다. 더 길면 같은 내용이 여러 `text` 프레임으로 나뉘어 온다(번호가 따로 붙는다).
- 일상 대화는 사람처럼 몇 초 간격으로 여러 프레임에 나눠 올 수 있다. 사용자가 관리 페이지에서 정한다.
- 다른 앱의 대화에 속한 프레임(다른 앱에서 한 말의 답, 11.1)에는 `conversation`(그 말을 한 앱의 대화 ID)이 붙는다. `note`·`files`·`card`·`card_closed`·`sticker`도 같다. 이 필드가 없으면 이 앱 대화나 앱 모두의 대화(`app:*`)의 프레임이다.

**`note`**: 진행 안내. 예: "메일을 찾아보는 중…". 작게 보이고 푸시로 깨우지 않는다.

```json
{"type": "note", "seq": 59, "body": "메일함을 확인하고 있어요"}
```

**`files`**: 파일이 붙은 메시지.

```json
{"type": "files", "seq": 60, "body": "정리했어요",
 "files": [{"id": "f_9c1e0a7b2d4f", "name": "정리.md", "size": 1834, "mime": "text/markdown", "kind": "text"}]}
```

- 파일은 `GET /v1/files/{id}`로 받는다(8.5). 30일 뒤 지워진다.
- 본문이 길면 앞부분은 `text` 프레임으로, 마지막 부분이 `files.body`로 온다.
- **파일 ID는 기록의 `files[].id`(6.2, `shape: 2`)와 같다.** 보관소에 이미 있는 파일(Lucia가 만든 것, Discord로도 보낸 것 등)은 사본을 만들지 않고 그 ID로 알리고, 이 대화가 받을 수 있게 한다. 같은 파일을 실시간 프레임과 기록에서 같은 ID로 맞춰 볼 수 있다.

**`card`**: 승인 카드(9.1).

```json
{"type": "card", "seq": 61, "ref": "app:cl-3f9a0c12de:5b0e1f3a9c2d", "card": { … }}
```

**`card_closed`**: 같은 `ref`의 카드가 닫혔다(승인됨·거절됨·만료됨·실패). 받은 `card`로 그 카드를 바꿔 그린다. 닫힌 카드에는 보통 버튼이 없고, `headline`이 결과 문구다.

```json
{"type": "card_closed", "seq": 62, "ref": "app:cl-3f9a0c12de:5b0e1f3a9c2d", "card": { … }}
```

> 다시 받기에서는 `card`와 `card_closed`가 차례로 온다. 순서대로 처리하면 닫힌 카드가 닫힌 채로 보인다.

**`sent`**(결정 0067): 사용자가 **다른 앱**에서 한 말의 사본. 한 앱이 `message`를 보내면 데몬은 그 앱을 뺀 모든 앱의 상자에 이 프레임을 넣는다(보낸 앱은 받지 않는다).

```json
{"type": "sent", "seq": 63, "body": "이거 번역해 줘",
 "conversation": "app:cl-3f9a0c12de", "from": {"client": "cl-3f9a0c12de", "name": "데스크톱 앱"},
 "at": "2026-11-22T08:12:03.512300+00:00",
 "files": [{"id": "f_9c1e0a7b2d4f", "name": "원문.md", "size": 1834, "mime": "text/markdown", "kind": "text"}],
 "sticker": {"id": "s_7a1c0e9b3f52", "name": "축하"},
 "context_note": "[고른 글 120자 함께 보냄 · msedge.exe]"}
```

| 필드 | 형식 | 설명 |
|---|---|---|
| `body` | 문자열 | 사용자가 보낸 글(앞뒤 공백을 지운 `message.text`). 글 없이 파일이나 스티커만 보냈으면 `""`다. 스티커 설명 줄, 파일 설명 줄, 고른 글 표지는 들어 있지 않다(아래). |
| `files` | 배열 | 붙은 파일(9.2의 파일 객체). 없으면 필드가 없다. 받는 길은 8.5. 짧게 둔 파일(`keep=short`, 8.4)은 다른 앱이 받지 못하므로 싣지 않는다. |
| `sticker` | 객체 | 보낸 스티커 `{id, name}`. 없으면 필드가 없다. |
| `context_note` | 문자열 | `message.context`(5.6)를 실었으면 그 표지 한 줄. 고른 글 자체는 오지 않는다. 없으면 필드가 없다. |
| `conversation` | 문자열 | 그 말을 한 앱의 대화 ID |
| `from` | 객체 | 그 말을 한 앱: `client`(클라이언트 ID), `name`(발급할 때 붙인 이름) |
| `at` | 문자열(RFC 3339) | 데몬이 말을 받은 시각 |

- **울리지 않고 푸시하지 않는다.** `alert` 필드도 없다. 내가 한 말이라 알릴 일이 없다.
- 상자에 남고 다시 받기(5.4)에도 온다. 앱마다 번호가 따로 붙는다.
- `history`(6.2)의 같은 말과 `body`가 다르다. 기록의 `body`에는 스티커 설명 줄(`[스티커:이름 — 설명]`), 파일 설명 줄(`[첨부: …]`, `shape: 2`이고 `files`가 있으면 빠진다), 고른 글 표지(`[고른 글 N자 함께 보냄 · 프로그램]`)가 이어 붙을 수 있다. `sent`는 글(`body`)과 이 셋(`sticker`, `files`, `context_note`)을 나눠 준다.
- Lucia의 답이나 Discord DM에서 한 말에는 `sent`가 없다. 사용자가 **앱에서** 한 말만 온다.

#### 번호 없는 프레임(상자에 남지 않는다)

| type | 필드 | 설명 |
|---|---|---|
| `typing` | 없음 | Lucia가 답을 쓰는 중. 몇 초마다 다시 올 수 있다. 다음 `text`가 오거나 몇 초 지나면 표시를 지운다. **연결된 앱 모두**에 간다. 어느 앱에서 한 말이든 같다(결정 0067). |
| `accepted` | `id`(말의 ID, 없으면 `null`) | 앱의 `message`·`event`를 받았다. 처리했다는 뜻은 아니다. 답은 나중에 `text`로 온다. |
| `result` | `id`, `ok`, `data` / `error` | 앱의 `call`에 대한 답(6장) |
| `invoke` | `id`, `method`, `params`, `timeout` | 데몬의 부탁(7장). `timeout`은 데몬이 기다리는 초(7.1). |
| `error` | `code`, `message` | 오류(15장). `message`는 사람용 한국어이고 바뀔 수 있다. 앱은 `code`로 가른다. |
| `pong` | 없음 | 앱의 `ping`에 대한 답 |
| `resumed` | 5.4 | 다시 받기의 끝 |
| `profile` | `name`(문자열), `avatar`(8.7의 객체 또는 `null`) | Lucia의 프로필 그림이나 이름이 바뀌었거나 그림이 없어졌다. 늘 지금의 **이름과 그림 전체**가 오니, 받은 값으로 둘 다 바꿔 그린다. 꺼져 있던 앱은 다음 `welcome.profile`에서 받는다. |
| `seen` | `seq` | 다른 기기에서 그 번호까지 봤다(11.1). `seq`는 **받는 앱의 번호**다. 앱은 그 번호까지의 알림(말풍선, 운영체제 알림, 깜박임)을 치운다. 되돌려 `seen`을 보내지 않는다. 끊겨 있던 앱에는 이 프레임 대신 내용 없는 푸시(`clear`, 10.4)가 간다. |
| `alert` | `seq` | 울리지 않고 놓아 둔(`alert: false`) 그 번호의 프레임을 지금 알려라(11.1). 앱이 이미 봤으면 아무 일도 없다. |
| `notes` | `rev` | 데몬의 메모가 바뀌었다(6.8). 앱은 `notes.list`의 `since`로 다시 읽는다. |
| `tasks` | 없음 | 맡은 일 목록이 바뀌었다(6.9). 앱은 `tasks.list`를 다시 부른다. 10초에 한 번까지 온다. |

### 5.6 앱 → 데몬 프레임

**`message`**: 사용자의 말.

```json
{"type": "message", "text": "내일 일정 알려 줘", "files": ["f_9c1e0a7b2d4f"], "id": "m-1700000000001"}
```

| 필드 | 형식 | 필수 | 설명 |
|---|---|---|---|
| `text` | 문자열 | `files`가 없으면 예 | 앞뒤 공백은 지운다. 8000자까지. |
| `files` | 문자열 배열 | 아니오 | 먼저 `POST /v1/files`로 올린 파일 ID. 10개까지. 이 앱이 올린 파일만 쓸 수 있다. |
| `id` | 문자열 | 아니오 | 앱이 붙인 ID. `accepted.id`로 돌아온다(보낸 말과 짝 맞추기). |
| `context` | 객체 | 아니오 | 말과 따로 주는 맥락(사용자가 화면에서 고른 글). `capabilities.message_context`가 있을 때만 보낸다. 아래. |

- 성공하면 `accepted`가 온다. 말한 것은 그 앞의 알림을 본 것으로 친다(`seen`, 5.6). 다른 앱의 상자에는 이 말의 사본이 `sent`(5.5)로 들어간다(결정 0067).
- 실패하면 오류가 온다:
  - `empty`: 글도 파일도 없다. `context`만 있어도 같다(`context`는 말에 딸린 것이다).
  - `too_long`: 글이 너무 길다.
  - `bad_file`: 쓸 수 없는 파일 ID다(`message`에 그 ID가 들어 있다).
  - `closed`: 데몬이 멈추는 중이다.

`context`(결정 0066):

```json
{"type": "message", "text": "이거 번역해 줘", "id": "m-1",
 "context": {"selection": "The quick brown fox…", "source": {"app": "msedge.exe", "title": "Example Domain"}}}
```

| 필드 | 형식 | 설명 |
|---|---|---|
| `selection` | 문자열 | 고른 글. 앞뒤 공백을 지우고 4000자까지 남긴다(넘으면 자른다). 비었으면 `context`가 없는 것으로 본다. |
| `source` | 객체 | 선택. `app`(프로그램 이름, 100자까지), `title`(창 제목, 200자까지). 줄바꿈과 연속 공백은 공백 하나로 합친다. 싣는 것은 사용자가 내주기로 한 경우에만 하는 것이 좋다(앱이 정한다). |

- 데몬은 `context`를 말과 따로 **외부 자료**로 Lucia에게 준다. 웹 페이지 같은 남의 글일 수 있어서, 그 안의 요청이나 지시는 따르지 않는다. 프로그램 이름과 창 제목도 외부 글이다.
- **기록에는 고른 글을 남기지 않는다.** 기록은 나중 요청에 사용자의 말로 다시 들어가기 때문이다(답장의 원문을 남기지 않는 것과 같은 까닭). 그 말의 본문 끝에 `[고른 글 4000자 함께 보냄 · msedge.exe]` 한 줄만 남는다(프로그램 이름이 있으면 40자까지, 없으면 `[고른 글 4000자 함께 보냄]`). `history`(6.2)로 읽으면 이 줄이 보인다.

**`press`**: 카드 버튼 누르기.

```json
{"type": "press", "ref": "app:cl-3f9a0c12de:5b0e1f3a9c2d", "action": "approve:apr-…", "input": "123456"}
{"type": "press", "ref": "app:cl-3f9a0c12de:5b0e1f3a9c2d", "action": "approve:apr-…",
 "inputs": {"username": "me", "password": "…", "save": true}}
```

| 필드 | 설명 |
|---|---|
| `ref` | 카드의 `ref`. `app:`으로 시작하는 카드면 **어느 앱의 카드든** 누를 수 있다(이 앱의 카드, 다른 앱 대화의 카드, 앱 모두의 카드 `app:*:`, 결정 0067·11.1). 앱 카드가 아니면 `bad_ref`. |
| `action` | 누른 버튼의 `action` 그대로 |
| `input` | 값을 하나 받는 버튼(`prompt`가 있는 버튼)만. 앞뒤 공백은 지운다. 서버는 로그에 남기지 않는다. |
| `inputs` | 여러 칸을 받는 버튼(`form`이 있는 버튼)만. 칸 이름(`fields[].key`) → 값 객체. 값은 글(문자열)이나 불이다. **값을 다듬지 않는다**(비밀번호의 앞뒤 공백도 값이다). 서버는 로그에 남기지 않는다. 칸은 8개까지, 칸 이름은 1~40자, 값은 1000자까지다. 어기면 `bad_frame`(`inputs`가 객체가 아니거나 값이 글도 불도 아니어도 같다). |

- 성공해도 바로 답이 없다. 결과는 `card_closed`로 온다. 누른 것은 그 앞의 알림을 본 것으로 친다(`seen`, 5.6).
- `form`이 있는 버튼을 `inputs` 없이 누르면 아무 일도 없다(카드는 열린 채 남는다). `prompt`가 있는 버튼은 `input`, `form`이 있는 버튼은 `inputs`로 보낸다. "취소"처럼 둘 다 없는 버튼은 값 없이 누른다.
- `inputs`가 있기만 하면(빈 객체 `{}`도) 값을 받은 것으로 보고 카드를 닫는다. 칸의 `min_len`·`max_len`은 데몬이 프레임에서 검사하지 않는다. 앱이 먼저 확인한다.
- 이미 닫힌 카드, 다른 곳에 올라간 카드, 모르는 `action`을 누르면 아무 일도 없다(조용히 무시).

**`ack`**: `{"type": "ack", "seq": 62}`(5.4). 답 없음.

**`presence`**: 앱이 앞에 있는지, 사람이 자리에 있는지 알린다. 답 없음. 값이 틀리면 `bad_frame`.

```json
{"type": "presence", "state": "background"}
```

| `state` | 뜻 |
|---|---|
| `foreground` | 앱이 앞에 있다 |
| `background` | 앱이 뒤에 있다. 데스크톱이면 창이 뒤에 있을 뿐 사람은 있을 수 있다. |
| `away` | 사람이 기기 앞에 없다(결정 0066). 무엇을 `away`로 칠지(키보드와 마우스를 한참 쓰지 않음, 화면 잠금, 운영체제가 알림을 막고 있음 등)는 앱이 정한다. `capabilities.presence_away`가 있을 때만 보낸다. |

- `background`와 `away`이면 연결이 살아 있어도, 그 앱이 울릴 앱으로 골라진 `text`·`files`·`card`는 푸시로 깨운다(10.2, 11.1).
- `away`는 울릴 곳을 가르는 데도 쓰인다. `away`가 되면 그 앱이 울려 놓고 보지 않은 알림이 바로 푸시를 받는 앱으로 넘어간다(11.1).
- 다시 앞으로 오면 `foreground`를 보낸다. `away`에서 돌아올 때는 `foreground`나 `background`를 보낸다(`away`는 저절로 풀리지 않는다).
- 새 연결은 늘 `foreground`로 시작한다. 다시 붙었는데 이미 뒤에 있거나 자리를 비웠으면 `welcome` 뒤에 지금 상태를 보낸다.

**`desk`**: 책상 상태. 사용자가 기기에서 "화면 보여 주기"나 "동행"(Lucia가 컴퓨터를 움직이기)을 켜고 끈 것을 알린다(결정 0066). 번호 없는 프레임이고 답은 없다. `capabilities.desk`가 있을 때만 보낸다.

```json
{"type": "desk", "showing": true, "controlling": false, "until": "2026-11-22T08:30:00Z"}
```

| 필드 | 형식 | 설명 |
|---|---|---|
| `showing` | 불 | 화면을 찍어 줄 수 있다. 빠지면 `false`. |
| `controlling` | 불 | 입력을 넣어 줄 수 있다(동행). 빠지면 `false`. |
| `until` | 문자열(RFC 3339) | 저절로 끝나는 때. 지나면 데몬은 둘 다 꺼진 것으로 본다. 둘 다 `false`이면 뺀다. 읽을 수 없는 값은 없는 것으로 본다. |

- 상태는 **연결마다** 둔다. 새 연결은 둘 다 꺼진 채로 시작하므로, 켜져 있으면 `welcome` 뒤에 다시 보낸다. 연결이 끊기면 꺼진 것으로 친다.
- 켜지고 꺼질 때(`showing`·`controlling`이 바뀔 때, 같은 상태를 다시 보내면 아무 일도 없다) 활동 기록에 남고, 그 앱 대화의 **상태 알림** 한 줄이 된다. `until`이 지나 저절로 꺼진 것은 그때 바로 남지 않고, 그 뒤 앱이 보내는 `desk`(꺼짐이든, 지난 `until`이든)에서 한 번 남는다. 다시 켜면 켬으로 남는다. 그래서 저절로 꺼지면 앱이 꺼짐을 한 번 보내 준다. Lucia의 `app_tools`는 지금 시각으로 보므로 그 사이에도 꺼진 것으로 보인다. 연결이 끊겨 꺼진 것은 남긴다. 상태 알림은 그 대화의 다음 요청에 한 번 Lucia에게 실린다(사용자가 한 말이 아니라는 표시와 함께). Lucia의 도구 `app_tools`는 앱마다 화면을 볼 수 있는지와 동행이 켜졌는지를 보인다.
- **이 프레임은 허락이 아니다.** 데몬이 화면 찍기를 막지도 열지도 않는다. 찍을지는 앱이 `invoke`가 올 때마다 그때그때 본다(7장).

**`seen`**: 이 앱의 사용자가 이 번호까지 **모두** 봤다(결정 0066). 번호 없는 프레임이고 답은 없다. `capabilities.seen`이 있을 때만 보낸다.

```json
{"type": "seen", "seq": 58}
```

- 앱이 여럿일 때 같은 알림을 다른 기기에서 또 울리지 않으려고 쓴다(11.1). 데몬은 `seen`을 받으면 그 알림의 넘기기를 그만두고, 같은 알림을 가진 다른 앱에 알린다(5.5의 `seen`, 10.4의 `clear`).
- `seq`는 번호 하나다. 앞의 것을 보지 않았으면 뒤의 것을 봤다고 알릴 수 없다. 마지막으로 매긴 번호보다 큰 값은 마지막 번호로 맞춘다.
- `seq`가 0 이하이면 아무것도 본 것으로 치지 않는다. 다만 이 앱이 `seen`을 말하는 앱이라는 것은 데몬에 알려진다(12.6).
- `press`와 `message`도 그 앞의 것을 본 것으로 친다(데몬이 알아서. `seen`을 한 번도 보내지 않는 앱도 그렇다).
- 언제 봤다고 할지는 앱이 정한다. **모자라게 알린다**(12.6).

**`call`**: 메서드 호출(6장). `{"type": "call", "id": "c1", "method": "history", "params": {...}}`

**`result`**: 데몬의 `invoke`에 대한 답(7장). 파일을 싣는 `files`가 있다(7.1).

**`event`**: 앱 연동의 이벤트. 예: 휴대폰 알림, 위치 도착.

```json
{"type": "event", "key": "noti-8812", "text": "택배: 문 앞에 두었습니다"}
```

| 필드 | 설명 |
|---|---|
| `key` | 선택. 같은 이벤트를 두 번 넣지 않기 위한 키(앞뒤 공백 제거, 120자까지). 없으면 데몬이 만든다. |
| `text` | 앞뒤 공백 제거, 1000자까지(넘으면 자른다) |

- 성공하면 `accepted`가 온다.
- 실패하면 오류가 온다:
  - `empty`: 빈 글이다.
  - `too_many_events`: 이 연결에서 한 시간 60개를 넘었다.
  - `unsupported`: 이벤트 입구가 없는 데몬이다.
- Lucia는 이 글을 **사람의 말이 아닌 외부 자료**로 다룬다. 신호 처리로 분류되고, 그 안의 지시를 따르지 않는다.

**`ping`**: `{"type": "ping"}`을 보내면 `pong`이 온다. 앱 쪽 생존 확인용이다.

모르는 `type`이나 깨진 JSON에는 `bad_frame`이 온다(연결은 그대로). 인증 뒤의 `hello`도 `bad_frame`이다.

### 5.7 생존 확인

- 데몬은 30초마다 WebSocket 핑(제어 프레임)을 보낸다. 표준 WebSocket 라이브러리는 자동으로 퐁을 보낸다.
- 데몬은 75초 동안 앱에서 **아무것도**(퐁 포함) 듣지 못하면 연결을 끊는다.
- 앱 쪽에서도 끊김을 빨리 알려면 30초 정도마다 `ping` 프레임을 보내고, `pong`이 10초 넘게 없으면 다시 연결한다.

---

## 6. 메서드(`call`)

### 6.1 호출과 답

```json
→ {"type": "call", "id": "c1", "method": "history", "params": {"limit": 50}}
← {"type": "result", "id": "c1", "ok": true, "data": {…}}
← {"type": "result", "id": "c1", "ok": false, "error": {"code": "forbidden", "message": "…"}}
```

- `id`는 앱이 정하는 문자열이다. 같은 연결 안에서 겹치지 않게 한다.
- `params`는 빼도 된다(`null`로 본다).
- 답은 다른 프레임과 섞여, 순서 없이 올 수 있다. `id`로 짝을 맞춘다.
- 연결이 끊기면 답은 오지 않는다. 앱은 다시 연결한 뒤 필요하면 다시 부른다.
- 부를 수 있는 메서드 목록은 `welcome.methods`에 있다.

### 6.2 `history`: 대화 기록

| params | 형식 | 기본 | 설명 |
|---|---|---|---|
| `before` | 정수 | 없음 | 이 기록 ID보다 앞의 것만 |
| `limit` | 정수 | 50 | 1~100(밖의 값은 맞춘다) |
| `scope` | 문자열 | `"app"` | `app`: **모든 앱 대화**(이 앱, 다른 앱, 취소한 앱의 지난 대화, 앱 모두에 둔 알림의 대화 `app:*`). 앱은 하나의 타임라인이라 이것이 앱이 보는 대화 전체다(결정 0067, 11장). `owner`: 여기에 Discord DM을 더한다. 서버 채널 대화는 넣지 않는다. |
| `shape` | 정수 | `1` | `2`이면 항목에 `parts`·`files`를 싣고 본문에서 파일 설명 줄을 뺀다(아래). `capabilities.history_shape`가 있을 때만 준다. 빼거나 `1`이면 예전 모양이다. 그 밖의 값은 `bad_params`. |

```json
{"items": [
   {"id": 1201, "at": "2026-11-19T00:10:00+00:00", "from": "user", "conversation": "app:cl-3f9a0c12de", "body": "내일 일정 알려 줘"},
   {"id": 1202, "at": "2026-11-19T00:10:09+00:00", "from": "lucia", "conversation": "app:cl-3f9a0c12de", "body": "내일은…"}
 ],
 "more": true}
```

`shape: 2`일 때의 항목(결정 0066):

```json
{"id": 1203, "at": "2026-11-22T00:10:09+00:00", "from": "lucia", "conversation": "app:cl-3f9a0c12de",
 "body": "정리해서 드릴게요.\n\n한 장으로 줄였어요.",
 "parts": ["정리해서 드릴게요.", "한 장으로 줄였어요."],
 "files": [{"id": "f_9c1e0a7b2d4f", "name": "정리.md", "size": 1834, "mime": "text/markdown", "kind": "text", "reachable": true},
           {"id": "f_0a1b2c3d4e5f", "name": "옛.md", "size": 90, "mime": "text/markdown", "kind": "text",
            "gone": true, "reachable": false}]}
```

| 필드 | 형식 | 설명 |
|---|---|---|
| `parts` | 문자열 배열 | 이 말이 여러 `text` 프레임으로 나뉘어 갔으면 그 조각들(보낸 차례). 하나로 갔으면 없다. `body`에는 이 조각이 빈 줄 하나(`\n\n`)로 이어져 있다(예전과 같다). 앱 대화의 조각은 스티커 표기가 `body`처럼 지워져 있다. |
| `files` | 배열 | 이 말에 붙은 파일. 없으면 필드가 없다. 항목은 파일 객체(9.2)에 `reachable`이 더해지고, 지워졌으면 `gone`도 붙는다. 아래. |

- 항목의 파일:
  - `id`, `name`, `size`, `mime`, `kind`는 **그 말이 갈 때의 값**이다. 파일이 지워져도 남는다.
  - `id`는 실시간 `files` 프레임(5.5)의 파일 ID와 같다. 앱은 같은 파일을 둘에서 맞춰 볼 수 있다.
  - `gone: true`: 보관 기간이 지나 지웠거나(30일), 짧게 둔 파일(8.4)의 시각이 지났다. 이때 `reachable`은 늘 `false`다.
  - `reachable`: 이 토큰이 `GET /v1/files/{id}`로 받을 수 있는가(8.5). 늘 들어 있다. 어느 앱 대화의 파일이든 받을 수 있다(결정 0067). 범위 `owner_files`가 없는 토큰은 Discord DM의 파일이 `false`이고, 다른 앱이 짧게 둔 파일(8.4)도 `false`다. 앱은 이것으로 "여기서는 받을 수 없어요"를 미리 보일 수 있다.
  - 사용자가 붙인 말(`from: "user"`)에도 `files`가 있다.
- `body`에서는 **`files`가 있는 항목에 한해** 본문 끝의 파일 설명 줄(`[첨부: …]`)이 빠진다. 두 번 보이지 않게 하려는 것이다. `parts`만 있고 `files`가 없는 항목의 본문은 그대로다.
- 모양은 **결정 0066이 들어간 뒤의 말에만** 남아 있다. 그전 기록은 `shape: 2`로 읽어도 `parts`·`files` 없이 본문 끝에 설명 줄이 그대로 있다. 앱은 예전 모양을 읽는 길(설명 줄을 이름만 있는 파일 줄로 그리기)을 남겨 둔다.
- 사용자가 `message.context`(5.6)를 실은 말의 본문 끝에는 `[고른 글 N자 함께 보냄 …]` 한 줄이 있을 수 있다. 고른 글 자체는 남지 않는다.

- 항목의 `conversation`은 그 말이 속한 대화다: 앱에서 한 말과 그 답은 말한 앱의 `app:<클라이언트 ID>`, 브리핑·알림은 `app:*`(앱 모두), Discord DM은 DM의 ID. `from: "user"` 항목의 `conversation`으로 어느 앱에서 한 말인지 알 수 있다(그 ID가 이 앱의 `welcome.conversation`이 아니면 다른 기기에서 한 말이다).
- `items`는 **오래된 것부터** 온다. `more`가 `true`이면 더 앞이 있다. 더 읽으려면 `before`에 첫 항목의 `id`를 넣는다.
- `from`:
  - `user`: 사용자
  - `lucia`: Lucia
  - `daemon`: 데몬이 남긴 기록(브리핑 등)
- 기록은 데몬의 대화 기록(에피소드)이다. 카드·파일 프레임 자체는 들어 있지 않다(파일은 `shape: 2`의 `files`로 알 수 있다).
  - 파일이 붙은 말은 본문 끝에 파일 설명 줄이 붙어 있을 수 있다(`shape: 2`이고 `files`가 있으면 빠진다).
  - 사용자가 보관 기간을 정했으면 오래된 기록은 지워진다.
- 오류: `bad_params`(`scope`·`shape` 값이 틀렸다), `failed`.

### 6.3 `commands`: 쓸 수 있는 명령

```json
{"commands": [{"name": "권한", "about": "권한 모드 보기·바꾸기",
               "options": ["엄격", "기본", "해제"], "current": "기본", "can_change": false},
              {"name": "모델", "about": "주 모델 보기·바꾸기",
               "options": ["fable", "opus", "sonnet", "haiku", "default"], "current": "default",
               "can_change": true}]}
```

- `options`는 `command`의 `option`으로 그대로 보낼 수 있는 값이다.
- `current`는 지금 값이고 `options` 가운데 하나다. `모델`의 `default`는 "고르지 않았다(데몬 설정 파일의 모델)"는 뜻이다.
  - 값은 다른 곳(관리 페이지, Discord)에서도 바뀐다. 바뀌었다는 프레임은 오지 않으므로, 고르는 화면을 열 때 `commands`를 다시 부른다.
- `can_change`는 이 토큰으로 바꿀 수 있는지다. `권한`은 `security_mode` 범위가 있을 때만, `모델`은 늘 `true`다.
- 명령은 늘어날 수 있다. 앱은 이 목록으로 화면을 그리고, 모르는 명령도 `options`를 고르는 모양으로 보이면 된다.

### 6.4 `command`: 명령 실행

| params | 설명 |
|---|---|
| `name` | `"권한"` 또는 `"모델"` |
| `option` | 선택. 없으면 지금 값을 보기만 한다. `권한`: `"엄격"`·`"기본"`·`"해제"`(또는 `strict`·`standard`·`open`). `모델`: `fable`·`opus`·`sonnet`·`haiku`, 설정 파일 값으로 되돌리려면 `default`(또는 `"기본"`). |

- 답: `{"text": "🛡️ 권한 모드: **기본** — …"}`. 마크다운 글이다.
- `모델`(결정 0055): Lucia의 대화·작업·브리핑이 쓰는 모델이다.
  - 바꾸면 다음 실행부터 적용되고 대화는 이어진다. 바꾼 뒤 첫 답은 평소보다 느릴 수 있다.
  - 대화가 길 때는 바꾸기 전에 `session.compact`(6.6)를 먼저 부르면 새 모델이 다시 읽을 양이 줄어든다.
  - 보기의 답은 지금 모델과 고를 수 있는 모델의 설명이다. 바꾼 것은 사용자의 알림 대화에도 기록된다.
- 오류:
  - `unknown_command`
  - `forbidden`: `권한`에 `option`이 있는데 `security_mode` 범위가 없다.
  - `timeout`: 5초 안에 답이 없었다. 그때는 적용되지 않는다.
  - `failed`, `closed`, `unsupported`
- 모드 값이 틀리면 오류가 아니라 안내 글이 `text`로 온다.
- 모드를 바꾸면 Discord DM(있으면)에 기록이 남는다.

### 6.5 `push.register` / `push.unregister`

| params | 설명 |
|---|---|
| `token` | FCM 등록 토큰. `[A-Za-z0-9:_.-]`만 쓸 수 있고 4096자까지다. |
| `platform` | `"android"` \| `"ios"` \| `"web"` |

- `push.register`의 답은 `{"registered": true, "available": <데몬 푸시 가능>}`이다. 오류는 `bad_params`, `failed`.
- `push.unregister`의 답은 `{"registered": false}`이다.
- HTTP `PUT`/`DELETE /v1/push`와 같다(8.6).

### 6.6 `session.compact`: 대화 정리하고 끝날 때까지 기다리기(결정 0056)

Lucia의 상주 대화를 요약해 줄인다(모델 1회). 답은 **정리가 끝난 뒤에** 온다. 주 모델을 바꾸기 전에 부르면 새 모델이 다시 읽을 대화가 줄어든다(6.4의 `모델`).

| params | 형식 | 기본 | 설명 |
|---|---|---|---|
| `min_tokens` | 정수(0 이상) | 0 | 대화 맥락이 이보다 작으면 정리하지 않고 `small`로 답한다. 0이면 크기를 보지 않는다. |

```json
→ {"type": "call", "id": "s1", "method": "session.compact", "params": {"min_tokens": 80000}}
← {"type": "result", "id": "s1", "ok": true,
   "data": {"outcome": "compacted", "pre_tokens": 128662, "context_tokens": 2778,
            "text": "대화를 정리했습니다(128,662→2,778 토큰)."}}
```

| `outcome` | 뜻 | 모델 호출 | `context_tokens` |
|---|---|---|---|
| `compacted` | 정리했다. `pre_tokens`는 정리 전 크기다 | 1회 | 정리 뒤 크기 |
| `unchanged` | 마지막 정리 뒤에 새 대화가 없어 다시 하지 않았다 | 없음 | 지금 크기 |
| `small` | 맥락이 `min_tokens`보다 작아 하지 않았다 | 없음 | 지금 크기 |
| `none` | 정리할 대화(시작된 세션)가 없다 | 없음 | 0 |
| `reset` | 정리하지 못해 새 세션으로 넘어갔다. 기억과 최근 대화로 이어 간다 | 1회(실패) | 0 |

- 다섯 경우 모두 `ok: true`이고, 그 뒤에 주 모델을 바꿔도 된다. `text`는 사용자에게 그대로 보여 줄 수 있는 글이다.
- **기다리는 시간**: 정리 자체는 보통 20~75초다. 다만 차례대로 돈다. Lucia가 답하는 중이거나 정기 작업이 돌면 그것이 끝난 뒤에 시작한다. 데몬은 실행 시간 제한의 두 배에 30초를 더한 만큼(운영 설정으로 630초) 기다린 뒤 `timeout`으로 답한다.
  - 앱은 이 메서드에 짧은 시간 제한을 걸지 않는다. 기다리는 동안 진행 표시를 보이고, 연결은 평소처럼 유지한다(핑에 답한다).
  - 정리하는 동안 사용자가 보낸 말은 정리가 끝난 뒤에 처리된다.
- 오류:
  - `busy`: 앱이 청한 정리가 이미 큐에 있거나 돌고 있다. 몇 초 뒤에 다시 부른다.
  - `timeout`: 제때 끝나지 않았다. 아직 시작하지 않은 요청은 버려지고(뒤늦게 돌지 않는다), 이미 시작한 정리는 끝까지 간다.
  - `bad_params`: `min_tokens`가 0 이상의 정수가 아니다.
  - `failed`: 정리를 돌리지 못했다(세션은 그대로다). `closed`, `unsupported`.
- **연결이 끊겼을 때**: 답은 오지 않지만 정리는 계속된다. 다시 연결해 한 번 더 부르면 된다. 이미 끝났으면 모델을 부르지 않고 `unchanged`가 오고, 아직 도는 중이면 `busy`가 온다. 그래서 다시 불러도 정리가 두 번 돌지 않는다.
- 정리는 **지금 주 모델**이 한다. 그래서 순서가 중요하다: 정리 → 모델 바꾸기. 반대로 하면 새 모델이 대화 전체를 먼저 읽는다.
- 범위는 필요 없다.

### 6.7 `browser.handover.*`: 브라우저 화면 넘겨받기(결정 0063)

로그인 도중 사이트가 사람 확인(캡차, 추가 보안 확인 등)을 띄우면 사용자가 그 화면을 직접 보고 푼다(결정 0017). 관리 페이지의 "브라우저 넘겨받기"와 같은 일을 앱에서 하는 메서드 넷이다.

- **범위 `browser_handover`가 있어야 한다**(3.4, 기본 꺼짐). 없으면 `welcome.methods`에 보이지 않고 불러도 `forbidden`이다.
- **브라우저를 켠 데몬에만 있다.** 브라우저가 꺼진 데몬은 `welcome.methods`에 넷 다 없고, 부르면 `unknown_method`다.
- 넘겨받는 동안 **Lucia의 모든 실행이 브라우저를 쓰지 못한다.** 쓰려 하면 "사람이 브라우저 화면을 넘겨받아 쓰고 있다"는 답을 받고, 사용자의 로그인 요청도 그동안은 시도하지 못한다.
- 한 번에 최대 15분이다(`max_seconds`). 지나면 데몬이 끝낸다(1분마다 확인, 다음 호출 때도 확인).
- 넘겨받기는 데몬 전체에 하나다. 관리 페이지에서 시작한 것도 이 메서드로 보이고 끝낼 수 있다.
- 시작과 끝은 활동 기록에 남는다. **넣은 글과 누른 자리는 기록하지 않는다.** 화면 이미지는 저장하지 않는다.
- 화면은 HTTP로 받는다(8.9).

#### `browser.handover.state`

상태만 본다(시작하지 않는다). `params`는 없다.

```json
→ {"type": "call", "id": "b1", "method": "browser.handover.state"}
← {"type": "result", "id": "b1", "ok": true,
   "data": {"active": true, "left_seconds": 812, "max_seconds": 900,
            "tabs": [{"id": "6F1A…", "url": "https://example.com/login", "title": "로그인"}]}}
```

| 필드 | 형식 | 설명 |
|---|---|---|
| `active` | 불 | 넘겨받는 중인가 |
| `left_seconds` | 정수 \| null | 남은 초. 넘겨받는 중이 아니면 `null` |
| `max_seconds` | 정수 | 한 번에 넘겨받을 수 있는 최대 시간(900) |
| `tabs` | 배열 | 넘겨받는 중일 때만 채운다(아니면 `[]`). 항목은 `{id, url, title}`이다. `id`는 해석하지 말고 화면 받기(8.9)와 `input`의 `tab`에 그대로 쓴다. `url`·`title`은 가림 필터를 거친다. |

- 새 창(본인인증 팝업 등)은 탭으로 남는다(결정 0017). 새 탭이 생겼는지는 `state`를 다시 불러 본다.

#### `browser.handover.start`

넘겨받는다. `params`는 없다. 답은 `state`와 같다.

- 이미 넘겨받는 중이면 아무것도 바꾸지 않고 지금 상태를 답한다(남은 시간을 다시 재지 않는다).
- 브라우저가 닫혀 있었으면 이 호출이 띄운다. 로그인하던 화면은 브라우저를 한동안 쓰지 않으면 닫혀 사라질 수 있다.
- 오류: `failed`(시작하지 못했다. `message`는 사람이 읽는 글).

#### `browser.handover.input`

넘겨받은 화면에 입력을 하나 보낸다.

| params | 형식 | 필수 | 설명 |
|---|---|---|---|
| `tab` | 문자열 | 예 | `state.tabs[].id` |
| `kind` | 문자열 | 예 | `click`, `text`, `key`, `scroll` 가운데 하나 |
| `x`, `y` | 정수 | `click`이면 예 | 누를 자리. 8.9의 이미지 위 픽셀 좌표 그대로다(브라우저 화면의 CSS 픽셀). 0 이상의 정수여야 하고, 소수나 음수는 `bad_params`다. |
| `text` | 문자열 | `text`이면 예 | 1~2000자. 지금 **초점이 있는 칸**에 넣는다(먼저 `click`으로 그 칸을 누른다). Enter는 누르지 않는다. |
| `key` | 문자열 | `key`이면 예 | `enter`, `tab`, `backspace`, `escape`, `up`, `down` 가운데 하나. 다른 값은 `failed`("모르는 키")다. |
| `dy` | 정수 | `scroll`이면 예 | 양수는 아래, 음수는 위. 화면 픽셀. ±3000을 넘으면 거절하지 않고 그 값으로 맞춘다. 스크롤은 화면의 (200, 200) 자리에서 굴린다(자리를 고르지 못한다). |

```json
→ {"type": "call", "id": "b2", "method": "browser.handover.input",
   "params": {"tab": "6F1A…", "kind": "click", "x": 412, "y": 288}}
← {"type": "result", "id": "b2", "ok": true, "data": {"sent": true}}
```

- 답은 `{"sent": true}`뿐이다. 화면이 바뀌었는지는 화면을 다시 받아 본다. 관리 페이지는 입력 뒤 0.4초쯤 기다렸다가 화면을 다시 받는다.
- `params`를 먼저 검사하고(`bad_params`), 그다음 넘겨받은 상태인지 본다(`not_active`).

#### `browser.handover.end`

넘겨받기를 끝낸다(그 뒤로 Lucia가 다시 브라우저를 쓴다). `params`는 없다. 넘겨받는 동안 열린 탭은 닫히고, 로그인 쿠키는 남는다.

```json
← {"type": "result", "id": "b3", "ok": true, "data": {"active": false, "warning": null}}
```

- 넘겨받는 중이 아니어도 성공이다. 앱은 화면을 떠날 때 늘 부른다.
- `warning`은 탭을 닫지 못하는 등 정리에 실패했을 때의 사람이 읽는 글이다(없으면 `null`). 넘겨받기는 끝난 것이다.

#### 오류

| `error.code` | 경우 |
|---|---|
| `forbidden` | 토큰에 `browser_handover` 범위가 없다 |
| `unknown_method` | 브라우저가 꺼진 데몬이다 |
| `bad_params` | `input`의 `params`가 틀렸다(`tab`이 없다, `kind`를 모른다, 좌표·글·`dy`가 없거나 틀렸다) |
| `not_active` | 넘겨받은 상태가 아니다(`input`). `start`를 먼저 부른다. 최대 시간이 지나 데몬이 끝낸 뒤에도 온다. |
| `failed` | 시작하거나 입력하지 못했다(없는 탭, 모르는 키 포함). `message`는 사람이 읽는 글이다. |
| `unavailable` | 브라우저를 쓸 수 없다. 지금 데몬에서는 `unknown_method`가 먼저 와서 WebSocket에서는 보이지 않는다(HTTP에서는 온다, 8.9). 오면 `failed`처럼 다룬다. |

#### 앱이 부르는 순서(권장)

```text
사용자가 "화면 풀기"를 연다
→ call browser.handover.start            (state를 돌려준다. tabs[0].id를 고른다)
화면이 앞에 있는 동안 되풀이한다(화면 받기는 1초쯤 간격 [제안], 앞 요청이 끝난 뒤에 다음을 보낸다):
  GET /v1/browser/handover/frame/{tab}   → 이미지를 그린다
  사용자가 누르면      call browser.handover.input {tab, kind: "click", x, y}  → 0.4초쯤 뒤 화면을 다시 받는다
  사용자가 글을 넣으면  먼저 그 칸을 click, 그다음 text(필요하면 key enter)
  몇 초마다            call browser.handover.state → 새 탭이 생겼는지, left_seconds
화면을 떠나거나 앱이 뒤로 가면: 화면 받기를 멈추고 call browser.handover.end
```

- 이미지를 줄여 그린다면 사용자가 누른 자리를 이미지의 실제 픽셀 크기로 되돌려 `x`, `y`로 보낸다.
- `left_seconds`를 보이고, `not_active`(WebSocket)나 `409`(HTTP)가 오면 데몬이 끝낸 것이므로 화면을 닫는다.
- 푼 뒤에는 `end`를 부르고, 로그인이 필요했던 일을 Lucia에게 다시 청한다.

### 6.8 `notes.list` / `notes.put` / `notes.remove`: 메모(결정 0066)

앱의 스티커 메모를 데몬도 갖는다. 앱이 꺼져 있어도 Lucia가 메모를 보고 붙일 수 있고(도구 `note_list`·`note_add`·`note_update`·`note_remove`), 앱이 둘이어도 같은 메모가 보인다. 앱은 제 사본을 데몬의 것과 맞춘다.

- **범위는 필요 없다**(메모는 소유자의 것이다). `welcome.methods`에 `notes.list`가 있으면 이 데몬이 메모를 맡는다.
- 메모는 40장까지(뗀 것은 세지 않는다), 글은 1~2000자다.
- 판(`rev`)은 메모 전체에서 하나씩 늘어난다. 앱은 마지막으로 본 판을 기억하고 그 뒤에 바뀐 것만 다시 읽는다.
- 자리·고정·"보았음" 같은 기기의 상태는 올리지 않는다(데몬은 모른다).
- Lucia에게 메모 글은 외부 자료로 보인다(붙여 넣은 남의 글일 수 있다).

메모:

```json
{"id": "n-7a89a834b9", "text": "우유 사기", "color": "yellow",
 "updated_at": "2026-11-22T07:41:00.123456Z", "by": "user", "removed": false}
```

| 필드 | 형식 | 설명 |
|---|---|---|
| `id` | 문자열 | `n-` 다음에 영문 소문자·숫자 6~16자. **만드는 쪽이 정한다.** 앱이 새 메모를 만들 때 정하고, Lucia가 만들면 데몬이 `n-` + 16진수 10자로 정한다. |
| `text` | 문자열 | 메모 글 |
| `color` | 문자열 | `yellow`, `pink`, `blue`, `green`, `white` |
| `updated_at` | 문자열 | 고칠 때마다 **반드시 달라지는 표**다(같은 순간에 두 번 고쳐도). 앱은 뜻을 읽지 않고 `base`로 되돌려 보내는 데만 쓴다. 기기의 시계와 견주지 않는다. |
| `by` | 문자열 | 마지막으로 고친 쪽: `user`(앱에서 올린 것) 또는 `lucia` |
| `removed` | 불 | 뗀 메모(묘비)면 `true`. 묘비도 글을 지니지만 앱은 그리지 않는다. |

#### `notes.list`

| params | 형식 | 설명 |
|---|---|---|
| `since` | 정수 | 선택. 이 판 **뒤에** 바뀐 메모만. 빼면 전부. 정수가 아니면 `bad_params`. |

```json
→ {"type": "call", "id": "n1", "method": "notes.list", "params": {"since": 41}}
← {"type": "result", "id": "n1", "ok": true, "data": {"rev": 43, "notes": [{…}, {…}]}}
```

- `rev`는 지금의 판이다(메모가 바뀐 적이 없으면 0). 앱은 이것을 다음 `since`로 쓴다.
- 뗀 메모도 `removed: true`로 온다. 다른 기기가 뗀 것을 알 수 있게 30일 동안 두고, 그 뒤 지운다(6시간마다 정리).

#### `notes.put`

만들거나 고친다. 그 `id`가 없으면 만들고, 있으면 고친다.

| params | 형식 | 필수 | 설명 |
|---|---|---|---|
| `id` | 문자열 | 예 | 위의 모양 |
| `text` | 문자열 | 예 | 1~2000자. 공백뿐이면 안 된다. 데몬은 글을 고쳐 쓰지 않는다. |
| `color` | 문자열 | 예 | 모르는 색은 오류가 아니라 `yellow`로 받는다. |
| `base` | 문자열 | 아니오 | 이 메모에 대해 데몬이 마지막으로 준 `updated_at`. 처음 올리는 것이면 뺀다. |

```json
← {"type": "result", "id": "n2", "ok": true, "data": {"conflict": false, "note": {…}, "rev": 44}}
← {"type": "result", "id": "n3", "ok": true, "data": {"conflict": true, "note": {…지금 데몬의 메모…}}}
```

- **충돌은 오류가 아니라 답이다.** `base`가 데몬의 지금 `updated_at`과 다르면 받아들이지 않고 `{"conflict": true, "note": 지금의 메모}`를 준다. 이때 `rev`는 없다.
- `base` 없이 올렸는데 그 `id`가 이미 있어도 충돌이다. 뗀 메모(묘비)도 같다. 뗀 메모를 같은 `id`로 되살리려면 묘비의 `updated_at`을 `base`로 준다.
- 충돌했을 때 앱은 데몬의 메모를 받아 그리고, 기기에서 글을 고친 것이면 그 글을 새 `id`의 새 메모로 올려 어느 쪽 글도 잃지 않게 하는 것이 좋다(앱이 정한다). 색만 바꾼 것이면 데몬의 것을 그대로 받아도 된다.
- 저장되면 `rev`는 이 고침이 만든 판이다.
- 오류:
  - `bad_params`: 인자가 없거나 문자열이 아니다, `id` 모양이 틀렸다, 글이 비었거나 2000자를 넘는다, 살아 있는 메모가 이미 40장인데 새로 만들거나 되살린다.
  - `failed`: 저장하지 못했다.

#### `notes.remove`

| params | 형식 | 필수 | 설명 |
|---|---|---|---|
| `id` | 문자열 | 예 | 뗄 메모. 문자열이 아니면 `bad_params`. |

```json
← {"type": "result", "id": "n4", "ok": true, "data": {"ok": true, "rev": 45}}
```

- 모르는 `id`나 이미 뗀 메모도 `ok`다. 그때는 판이 그대로다.
- 뗀 메모는 묘비로 남는다(위).

#### `notes` 프레임

```json
{"type": "notes", "rev": 45}
```

- 메모의 판이 바뀔 때마다(앱이 올린 것, Lucia가 붙이거나 고치거나 뗀 것) 연결된 **앱 모두**에 간다. 올린 앱도 받는다. 앱은 제 고침이 이미 반영됐음을 판으로 알아본다.
- 충돌, 잘못된 인자, 없는 메모를 뗀 것은 판이 바뀌지 않으므로 오지 않는다.
- 번호 없는 프레임이라 상자에 남지 않는다. 꺼져 있던 앱은 연결할 때 `notes.list`로 읽는다.
- 앱은 `rev`가 자기가 아는 판보다 클 때 `notes.list`의 `since`로 다시 읽는다.

### 6.9 `tasks.list`: 맡은 일(결정 0066)

Lucia가 맡은 목표·작업·후속 조치의 진행을 한 목록으로 준다. 긴 작업의 진행은 Discord의 작업 스레드와 관리 페이지에만 있었다. **범위는 필요 없다**(소유자의 일이다). `welcome.methods`에 `tasks.list`가 있으면 쓸 수 있다. `params`는 없다.

```json
→ {"type": "call", "id": "c7", "method": "tasks.list"}
← {"type": "result", "id": "c7", "ok": true, "data": {"tasks": [
     {"id": "g-12", "kind": "goal", "title": "분기 보고서 초안", "state": "running",
      "note": "자료 3개 가운데 2개를 읽었어요", "updated_at": "2026-11-22T07:41:00Z"}]}}
```

| 필드 | 형식 | 설명 |
|---|---|---|
| `id` | 문자열 | `g-`(목표)·`t-`(작업)·`f-`(후속 조치) 다음에 저장소의 ID. 해석하지 않는다. |
| `kind` | 문자열 | `goal`(목표), `task`(작업), `followup`(후속 조치) |
| `title` | 문자열 | 한 줄, 120자까지(넘으면 끝에 `…`). 가림 필터를 거친다. |
| `state` | 문자열 | 아래 |
| `note` | 문자열 \| null | 마지막 진행 한 줄(200자까지). 끝났거나 확인을 기다리는 작업은 그 결과 한 줄. 사람에게 보이는 글이다. |
| `updated_at` | 문자열 | RFC 3339(초 단위, UTC) |

| `state` | 뜻 |
|---|---|
| `running` | 하는 중 |
| `waiting` | 사람을 기다린다(승인, 입력, 확인) |
| `scheduled` | 때를 기다린다(후속 조치) |
| `done`, `failed`, `cancelled` | 끝났다 |

원천이 `state`가 되는 대응:

| 원천 | 값 | `state` |
|---|---|---|
| 목표 | 진행 | `running` |
| 목표 | 기다림(사람 차례) | `waiting` |
| 목표 | 이룸 | `done` |
| 목표 | 그만둠 | `cancelled` |
| 작업 | 만듦, 도는 중 | `running` |
| 작업 | 승인·입력·확인(보고) 기다림 | `waiting` |
| 작업 | 끝남, 실패, 취소 | `done`, `failed`, `cancelled` |
| 작업 | 만료(사람이 끝내 답하지 않았다) | `cancelled` |
| 후속 조치 | 열림, 때가 오지 않음 | `scheduled` |
| 후속 조치 | 열림, 때가 지남(하는 중) | `running` |
| 후속 조치 | 끝남, 취소, 실패 | `done`, `cancelled`, `failed` |

- 열려 있는 것과 **지난 24시간 안에 끝난 것**을 준다. **30개까지**다. 끝나지 않은 것이 먼저고, 그 안에서 최근에 바뀐 것부터다.
- 오류: `failed`(목록을 만들지 못했다).

`tasks` 프레임:

```json
{"type": "tasks"}
```

- 목록이 바뀌었다는 알림이다. 내용은 없고, 앱이 `tasks.list`를 다시 부른다.
- 데몬이 **10초마다** 목록을 보고 지난번 알린 것과 달라졌으면 연결된 앱 모두에 보낸다(앱이 하나도 붙어 있지 않으면 보지 않는다). 쓰는 곳마다 알리지 않으므로, 진행 줄이 자주 바뀌어도 10초에 한 번까지만 온다.
- 번호 없는 프레임이라 상자에 남지 않는다. 앱은 연결할 때와 `tasks` 프레임마다 읽는다.

---

## 7. 데몬의 부탁(`invoke`)

앱에서만 할 수 있는 일(위치, 휴대폰 알림 읽기, 카메라 등)을 데몬이 앱에 부탁하는 길이다.

```json
← {"type": "invoke", "id": "d-7a1c0e9b3f52", "method": "location.get", "params": {"precise": true}, "timeout": 40}
→ {"type": "result", "id": "d-7a1c0e9b3f52", "ok": true, "data": {"lat": 37.5, "lng": 127.0}}
→ {"type": "result", "id": "d-7a1c0e9b3f52", "ok": false, "error": {"code": "denied", "message": "사용자가 거절"}}
```

### 7.1 부탁과 답

- 데몬은 앱이 `hello.features`로 알린 기능만 부른다. 기능 이름은 메서드 이름의 첫 마디다(`location.get` → `location`).
- 앱은 같은 `id`로 `result`를 한 번 보낸다. 오류의 `code`는 앱이 정한다.
  - 권장 코드: `denied`(사용자·OS가 거절), `timeout`(사용자가 제때 답하지 않았다), `unavailable`(지금 할 수 없음), `bad_params`, `failed`.
  - 코드는 Lucia에게 그대로 보인다. **`denied`와 `timeout`을 가려 쓴다.** "허락하지 않았다"와 "사용자가 보지 못했다"는 다른 말이고, Lucia가 사용자에게 정확히 전할 수 있다.
- **`invoke.timeout`**(초, 정수)은 데몬이 이 부탁의 답을 기다리는 시간이다. 앱은 그 안에 답한다.
  - 기능이 `tools[].timeout`(7.2)을 알렸으면 그 값(1~120)이고, 알리지 않았으면 40이다.
  - `capabilities.tool_timeout`이 있는 데몬이 보낸다. 없는 예전 데몬은 이 필드 없이 40초를 기다린다.
- 시간이 지나도 답이 없으면 데몬은 그 부탁을 버린다. 그 뒤에 온 늦은 답은 `bad_ref` 오류를 받는다. 데몬이 시간 안에 답을 받지 못했으면 Lucia는 `timeout`을 받는다(앱이 `timeout`으로 답한 것과 같다).
- 기다리는 부탁이 아닌 `id`로 답하면 `bad_ref`가 온다. 연결이 끊기면 데몬 쪽 부탁은 실패로 끝난다.
- `result.data`의 글은 Lucia가 외부 자료로 다룬다.
- 사용자에게 물어야 하는 일(권한 창, 동작마다 묻기)은 `tools[].timeout`을 늘려 알리고(최대 120초) 그 안에서 기다린다. 그래도 끝나지 않으면 `timeout`이나 `unavailable`, `denied`로 답하고, 다음 호출에서 되게 한다.
- **`result.files`**: 답에 파일을 싣는다(결정 0066). `capabilities.tool_files`가 있을 때만 싣는다.

  ```json
  → {"type": "result", "id": "d-…", "ok": true, "data": {"width": 2560, "height": 1440}, "files": ["f_b7f4006bdf27"]}
  ```

  - **이 앱이 올린 파일**(`POST /v1/files`, 8.4)의 ID만 쓸 수 있다. 남의 파일이나 없는 ID, 넷을 넘는 것은 빼고, 뺀 수를 Lucia에게 알린다. 4개까지다.
  - 오류 답(`ok: false`)의 `files`는 쓰지 않는다.
  - Lucia에게는 `data`와 함께 파일이 보인다. 그림은 3장·합계 3 MB까지 그림으로 보이고, 그 밖의 파일(그림이 아니거나 한도를 넘는 것)은 ID와 이름·종류·크기, "file_view로 본다"는 안내가 간다. 그래서 화면을 보여 주려고 Lucia가 `file_view`를 한 번 더 부를 필요가 없다.
  - 화면 사본처럼 남기지 않을 파일은 `keep=short`로 올린다(8.4).
- **Lucia가 기다리는 법**(앱은 신경 쓰지 않아도 된다): Lucia의 도구 호출(MCP 요청)은 60초쯤에 끊기므로 `app_call`은 45초까지만 기다린다. 그 안에 앱이 답하지 않으면 부탁 ID(`d-…`)를 받고, 도구 `app_result`로 결과를 본다(그때도 45초까지 더 기다린다). 데몬은 그동안 기능의 `timeout`까지 계속 기다리고, 답이 오면 결과를 잠시(10분쯤) 남겨 둔다. 앱 쪽에서는 `invoke`가 한 번 가고 `result`가 한 번 오는 것뿐이다.

### 7.2 Lucia에게 기능 내주기(`hello.tools`, 결정 0053)

앱은 자기가 할 수 있는 일을 `hello.tools`로 알린다. Lucia는 그 목록을 보고(도구 `app_tools`) 필요할 때 부른다(도구 `app_call` → 위의 `invoke`). **무엇을 내줄지는 앱이 정한다.** 데몬은 미리 정해 둔 기능 목록이 없다.

```json
"tools": [
  {"name": "location.get", "description": "휴대폰의 현재 위치(위도·경도·정확도)", "params": {"precise": "boolean, 정밀 위치(기본 false)"}},
  {"name": "notifications.recent", "description": "최근 휴대폰 알림 목록", "params": {"limit": "1~50"}},
  {"name": "alarm.set", "description": "알람 맞추기", "params": {"time": "HH:MM", "label": "string"}, "effect": "write"},
  {"name": "screen.capture", "description": "PC 화면을 찍어 올리고 파일 ID를 준다", "params": {"target": "screen | window"},
   "when": "here", "timeout": 90}
]
```

| 필드 | 형식 | 규칙 |
|---|---|---|
| `name` | 문자열 | `기능.동작`(마디 둘이나 셋). 마디는 `[a-z0-9_-]` 40자까지. 어기는 것은 버린다. 겹치면 처음 것만. |
| `description` | 문자열 | Lucia가 읽고 고르는 설명. 한 줄로 정리되고 200자까지. **언제 쓰는지와 무엇이 돌아오는지**를 적는다. |
| `params` | JSON | 인자 설명. 모양은 자유다(JSON Schema든 `{"이름": "설명"}`이든). Lucia에게 그대로 보인다. 직렬화해 2 KB를 넘으면 버린다. |
| `effect` | 문자열 | `"write"`면 무엇을 바꾸는 기능이다(알람 맞추기, 메시지 보내기 등). 빼면 읽기다. **본뜻(바꾸는가)만** 말한다. 사람이 있어야 하는 읽기(화면, 클립보드 등)는 `when`으로 말한다. |
| `when` | 문자열 | 언제 부르는가(결정 0066). 아래. 빼거나 문자열이 아니면 없는 것으로 본다. |
| `timeout` | 숫자 | 이 기능이 바라는 답 시간(초)(결정 0066). 소수는 올림하고 1~120으로 맞춘다. 빼거나 0 이하·숫자가 아니면 40이다. `invoke.timeout`으로 돌아온다(7.1). |

- 32개까지 받는다. 받아들인 이름은 `welcome.tools`로 돌아온다.
- `tools`에 적은 이름의 첫 마디는 `features`에 따로 적지 않아도 된다(데몬이 더한다).
- **사용자 허락**: 관리 페이지 "연결"에서 그 앱 토큰에 범위 `app_tools`("Lucia가 이 앱의 기능 부르기 허용")를 켜야 Lucia가 부른다. 기본은 꺼져 있다. `welcome.scopes`에 `app_tools`가 있는지로 안다. 없으면 앱이 "관리 페이지에서 허용해 달라"고 안내하면 좋다.
- **읽기와 쓰기, 부르는 때(`when`)**:
  - `when`이 없으면: 읽기 기능은 Lucia가 사람 없이 도는 일(브리핑 등)에서도 부를 수 있다. 원치 않으면 아래 `when`을 쓰거나 앱에서 거절한다. `effect: "write"`는 사용자가 Lucia와 대화 중일 때만 부른다.
  - `"attended"`: 읽기든 쓰기든 **사용자가 Lucia와 대화 중인 실행에서만** 부른다.
  - `"here"`: 사용자가 **이 앱의 대화**에서 말하고 있을 때만 부른다. Discord나 다른 앱에서 말하는 중에는 부르지 않는다(앱 대화가 하나로 보여도 이 판단은 말한 앱을 그대로 본다, 결정 0067). 화면 찍기, 컴퓨터 조작처럼 눈앞에서 일어나야 하는 것에 맞다.
  - 모르는 문자열은 가장 좁게 `here`로 본다.
  - 데몬의 "사람이 있는가"는 **어느 대화든** 사람이 말하고 있으면 참이다. 그래서 다른 앱에 사람이 있어도 내 기능이 불리지 않게 하려면 `here`가 필요하다.
  - 여러 앱이 같은 기능을 내주면 이 조건을 통과하는 앱을 부른다. 부를 수 없는 때 Lucia는 `forbidden`을 받는다(앱에는 아무것도 가지 않는다).
  - **`when`은 `capabilities.tool_when`이 있는 데몬만 지킨다.** 표시가 있을 때만 읽기 기능에서 `effect`를 빼고 `when`을 싣는다. 표시가 없는 데몬에 사람이 있어야 하는 읽기를 `effect` 없이 내주면 사람 없는 실행도 부를 수 있게 되므로, 그때는 `effect: "write"`로 내준다.
  - 휴대폰의 권한(위치 권한 등)과 사용자 확인은 앱이 맡는다. 거절이면 `denied`로 답한다.
- **연결돼 있을 때만**: 앱이 끊겨 있으면 Lucia는 "앱이 연결되어 있지 않다"는 답을 받는다. 데몬이 푸시로 깨워서 부르지는 않는다 [제안: 필요하면 더한다].
- **결과**: `result.data`는 JSON이면 무엇이든 된다. Lucia에게 8 KB까지 보인다(넘으면 잘린다). 외부 자료로 다뤄지므로 지시처럼 읽히는 글을 넣어도 따르지 않는다. 파일은 `result.files`로 싣는다(7.1).
- **기다리는 시간**: 사용자에게 묻고 하는 기능은 `timeout`(최대 120초)으로 알린다. Lucia의 `app_tools`에는 40초보다 긴 기능에 "답을 N초까지 기다린다"가 보인다.
- 다시 연결하면 `tools`를 다시 보낸다(연결마다 새로 정한다). 앱이 기능을 더하거나 빼면 다음 `hello`부터 반영된다.

---

## 8. HTTP API

공통:
- 기준 주소는 2.1에 있다.
- 인증은 `Authorization: Bearer <토큰>`이다(`/v1/health` 빼고).
- 응답 본문은 JSON이다(파일 받기 빼고).
- 오류 본문은 늘 아래 모양이다.

```json
{"error": {"code": "unauthorized", "message": "토큰이 틀렸거나 취소됐다"}}
```

공통 오류:

| 상태 | code | 경우 |
|---|---|---|
| 400 | `forbidden` | 출처를 알 수 없다(2.2) |
| 403 | `forbidden` | 받지 않는 출처, 또는 기기 역할 토큰 |
| 401 | `unauthorized` | 토큰이 없거나 틀렸거나 취소됐다 |
| 429 | `too_many_attempts` | 그 주소가 막혀 있다(3.3) |

### 8.1 `GET /v1/health`

인증 없음. `200`, 본문 `ok`(text/plain).

### 8.2 `GET /v1/me`

```json
{"v": 2, "client": "cl-3f9a0c12de", "name": "휴대폰 앱", "conversation": "app:cl-3f9a0c12de",
 "scopes": [], "methods": ["command", "commands", "history", "notes.list", "notes.put", "notes.remove",
                            "push.register", "push.unregister", "session.compact", "tasks.list"],
 "seq": 62, "acked": 60, "online": true,
 "push": {"available": true, "registered": true}}
```

- `seq`: 마지막으로 매긴 번호
- `acked`: 앱이 마지막으로 받았다고 알린 번호
- `online`: 지금 WebSocket이 붙어 있다
- `profile`: `welcome.profile`과 같다(위 예시에는 뺐다). 없으면 그 기능이 없는 데몬이다.

### 8.3 `GET /v1/history`

질의 `before`, `limit`, `scope`, `shape`는 6.2와 같다. 답도 같다. 오류는 `400`과 `bad_params`. `shape`가 정수가 아니면(`shape=abc`) 프레임워크의 기본 `400`이고 본문 형식이 다르다.

### 8.4 `POST /v1/files?name=<파일 이름>[&keep=short]`

- **본문이 파일 그대로**다(multipart 아님). `Content-Type`은 보지 않는다. 종류는 파일 앞부분의 서명으로 정한다.
- `name`은 퍼센트 인코딩한다. 빼면 `file`이다. 서버가 이름을 다듬는다:
  - 경로 앞부분을 지운다.
  - 제어 문자와 `/ \ : * ? " < > |`는 `_`로 바꾼다.
  - 앞의 `.`을 지운다.
  - 100자가 넘으면 줄인다.
  - 다듬은 이름이 답으로 온다.
- 크기는 31,457,280바이트(30 MiB)까지다. 빈 파일은 받지 않는다.
- 토큰을 확인한 뒤에야 본문을 읽는다.
- `keep`(선택, 결정 0066): `short`이면 **짧게 둔다**(아래). `capabilities.short_files`가 있을 때만 쓴다. 비워 두거나 `normal`이면 평소와 같다. 그 밖의 값은 `400 bad_params`다.

| 상태 | 본문 |
|---|---|
| 201 | `{"id", "name", "size", "mime", "kind"}`(9.2). `keep=short`이면 `expires_at`도 있다. |
| 400 | `bad_file`(빈 파일, 크기 초과 등), `bad_params`(`keep` 값이 틀렸다) |
| 413 | `too_large`(본문이 상한을 넘었거나 끊겼다) |
| 501 | `unsupported`(파일을 받지 않는 데몬) |

- 올린 파일은 **이 앱의 대화로만** 쓸 수 있다. `message.files`로 붙이면 Lucia가 연다:
  - 이미지·PDF는 10 MB까지 그대로 본다.
  - 텍스트·Office 문서는 글로 뽑아 읽는다.
  - 그 밖의 종류는 받았다는 사실만 안다.
- 30일 뒤 지운다(`keep=short`이면 1시간 뒤).

**짧게 두는 올리기(`keep=short`)**: 화면 사본처럼 파일로 오래 돌아다니면 안 되는 것에 쓴다.

```json
← 201 {"id": "f_b7f4006bdf27", "name": "screen.png", "size": 482113, "mime": "image/png", "kind": "image",
       "expires_at": "2026-11-22T08:41:00.123456789+00:00"}
```

- **1시간 뒤 지운다** [제안]. 데몬이 한 시간마다 정리하고, 그 전이라도 `expires_at`이 지나면 없는 파일로 본다(`GET /v1/files/{id}`는 `404 not_found`, `410`이 아니다). `expires_at`은 RFC 3339다. 평소 올린 파일에는 `expires_at`이 없다.
- Lucia가 파일을 다루는 길에서 빠진다:
  - 최근 파일 목록(`file_view`)
  - 보관함(`file_keep`은 거절한다)
  - 답에 붙여 내보내기(`report_result`의 파일)
  - 작업 공간으로 꺼내기(`file_export`)
- **파일 ID를 아는 실행만** 본다(`result.files`로 받은 `app_call` 등). 앱 자신은 그동안 `GET /v1/files/{id}`로 받을 수 있다. 다른 앱은 받지 못한다(8.5, 화면 사본이라 타임라인에 나오지 않는다).

### 8.5 `GET /v1/files/{id}`

- 받을 수 있는 파일:
  - **어느 앱 대화로 오간 파일이든**(앱이 올린 것, 데몬이 보낸 것): 이 앱의 대화, 다른 앱의 대화, 앱 모두에 둔 알림의 대화(`app:*`, 11.1). 앱 대화는 하나라서(결정 0067) 휴대폰에서 올린 사진을 데스크톱이 받고, 데스크톱 대화로 온 Lucia의 파일을 휴대폰이 받는다. 범위가 필요 없다.
  - 다만 다른 앱이 **짧게 둔 파일**(`keep=short`, 8.4)은 그 앱만 받는다. 화면 사본이라 타임라인에 나오지 않는다. 앱 기능의 답에 실린 파일(`result.files`, 7.1)은 짧게 두지 않았으면 다른 앱도 받는다.
  - 데몬이 이 대화로 알린 파일. 보관소에 이미 있는 파일을 다시 보내면 사본을 만들지 않고 그 대화가 받을 수 있게 한다(같은 ID, 5.5). 기록(`history`)에 붙은 파일도 그 말의 대화가 받을 수 있다.
  - **범위 `owner_files`(3.4)가 있으면** Discord 알림 대화(DM)의 파일도 받는다. 이 범위는 이제 DM의 파일에만 필요하다. 서버 채널 같은 그 밖의 대화의 파일은 범위가 있어도 받지 않는다.
  - 이 토큰이 받을 수 있는지는 기록의 `files[].reachable`로 미리 안다(6.2).
- `200`이면 본문이 파일이다. 머리:
  - `Content-Type: <mime>`
  - `Content-Disposition: attachment; filename*=UTF-8''<퍼센트 인코딩 이름>`
  - `X-Content-Type-Options: nosniff`
  - `Cache-Control: private, no-store`
- `404 not_found`: 없거나, 이 토큰이 받을 수 없는 파일이다(범위가 없는 토큰의 Discord DM 파일, 다른 앱이 짧게 둔 파일). 시각이 지난 짧게 둔 파일(8.4)도 같다.
- `410 gone`: 보관 기간이 지나 지웠다.

### 8.6 `PUT /v1/push`, `DELETE /v1/push`

```
PUT /v1/push
Content-Type: application/json

{"token": "<FCM 등록 토큰>", "platform": "android"}
```

- `PUT`:
  - 성공하면 `200 {"registered": true, "available": <불>}`.
  - 형식이 틀리면 `400 bad_params`.
  - `Content-Type`이 JSON이 아니면 `415`, JSON이 깨졌으면 `400`/`422`(프레임워크 기본 응답, 본문 형식이 다르다).
- `DELETE`: `200 {"registered": false}`.
- WebSocket 없이 부를 수 있다. 백그라운드에서 FCM 토큰이 바뀌었을 때(`onNewToken`) 쓴다.

### 8.7 `GET /v1/profile`: Lucia의 프로필(결정 0060, 0064)

프로필 그림의 정보와 보이는 이름이다. `welcome.profile`, `/v1/me`의 `profile`, `profile` 프레임과 같은 모양이다. 그림은 Lucia가 도구로 정하거나 사용자가 관리 페이지에서 정한다(결정 0064).

```json
{"name": "Lucia",
 "avatar": {"id": "a_cfc0dacd5dcd71abf496c418", "mime": "image/png", "width": 800, "height": 800,
            "size": 579183, "updated_at": "2026-11-20T04:43:07Z"}}
```

| 필드 | 형식 | 설명 |
|---|---|---|
| `name` | 문자열 | 관리 페이지에서 사용자가 정한 이름(1~24자, 한 줄). 정하지 않았으면 `"Lucia"`. 앱의 화면에 보이는 이름이다. Lucia가 자기를 부르는 이름과 말투(캐릭터), 알림 글에 적히는 글은 이것과 따로다. |
| `avatar` | 객체 또는 `null` | `null`이면 정한 그림이 없다. 앱의 기본 모습을 쓴다. |
| `avatar.id` | 문자열 | 그림의 ID(`a_` + 16진수 24자). **내용 해시**라서 그림이 바뀌면 ID가 바뀐다. 캐시 키로 쓴다. |
| `avatar.mime` | 문자열 | `image/png` 또는 `image/jpeg`. 움직이는 그림은 없다. |
| `avatar.width`, `avatar.height` | 정수 | 픽셀. 늘 정사각형이고 한 변은 128~2048이다. |
| `avatar.size` | 정수 | 바이트(3 MiB까지) |
| `avatar.updated_at` | 문자열 | 정한 시각(RFC 3339, UTC) |

- `200`이고 `Cache-Control: private, no-cache`다.
- `501 unsupported`: 프로필 기능이 없는 데몬이다.

### 8.8 `GET /v1/profile/avatar/{id}`: 프로필 그림

- `200`이면 본문이 그림이다. 머리:
  - `Content-Type: image/png` 또는 `image/jpeg`
  - `ETag: "<id>"`
  - `X-Content-Type-Options: nosniff`
  - `Cache-Control: private, max-age=31536000, immutable`
- ID가 내용 해시이므로 **한 번 받은 그림은 다시 받을 필요가 없다.** 앱은 ID를 키로 디스크에 캐시한다.
- 예전 ID의 그림도 한동안(최근 20개) 받을 수 있다. 지워진 ID는 `404 not_found`다.
- 다른 요청과 같이 `Authorization: Bearer <토큰>`이 있어야 한다. 그림을 주소로 바로 그리는 이미지 로더를 쓰면 **인증 머리를 붙이게 설정**하거나, 앱이 직접 받아 캐시한 파일을 그린다.

### 8.9 `GET /v1/browser/handover/frame/{tab}`: 넘겨받은 브라우저 화면(결정 0063)

넘겨받는 중인 브라우저의 화면 한 장이다. 범위 `browser_handover`가 있는 토큰만 받는다. `{tab}`은 `browser.handover.state`의 `tabs[].id`다(뒤에 `.jpg`를 붙여도 된다. 떼고 읽는다).

- `200`이면 본문이 JPEG다. 머리:
  - `Content-Type: image/jpeg`
  - `Cache-Control: no-store`
  - `X-Content-Type-Options: nosniff`
- 이미지는 브라우저의 보이는 화면 크기 그대로다. **이미지 위의 픽셀 좌표가 `browser.handover.input`의 `click` 좌표와 같다.** 크기는 응답마다 다를 수 있으니 받은 이미지의 실제 크기를 쓴다.
- 오래 두고 쓰는 그림이 아니다. 디스크에 캐시하지 않고, 화면을 떠나면 버린다.
- 다른 요청과 같이 `Authorization: Bearer <토큰>`이 있어야 한다. 이미지 로더를 쓰면 인증 머리를 붙이게 하거나 앱이 직접 받아 그린다.

| 상태 | code | 경우 |
|---|---|---|
| 403 | `forbidden` | 토큰에 `browser_handover` 범위가 없다 |
| 404 | `unavailable` | 브라우저가 꺼진 데몬이다 |
| 409 | `not_active` | 넘겨받은 상태가 아니다. `browser.handover.start`를 먼저 부른다. |
| 404 | `not_found` | 없는 탭이다(화면을 얻지 못한 경우도 같다) |

호출 순서와 간격은 6.7의 "앱이 부르는 순서"를 본다.

### 8.10 `POST /v1/voice`: 받아쓰기(결정 0066: 하지 않음)

데스크톱 앱의 "누르고 말하기"를 위해 길만 열어 두었다. **이 데몬에는 받아쓰기가 없다.** 외부 서비스를 붙이면 소리가 집 밖으로 나가므로 따로 정할 일이다.

| 상태 | code | 경우 |
|---|---|---|
| 501 | `unsupported` | 인증을 통과한 요청에는 늘 이 답이다(본문을 읽지 않는다) |

- 인증과 출처 확인은 다른 요청과 같다(3.2, 2.2). 토큰이 틀리면 `401`이다.
- `welcome.capabilities.voice`가 **없다.** 앱은 이 표시가 있을 때만 소리를 올리고, 없으면 기기의 받아쓰기를 쓴다.
- 요청·응답 모양은 정해지지 않았다. 앱 팀의 제안(`docs/proposals/desk-app-api.md` 12번: Ogg Opus나 WAV, 60초·5 MB, 답 `{"text": "…"}`)은 제안일 뿐이고, 받아쓰기를 붙일 때 정한다.

---

## 9. 데이터 형식

### 9.1 카드(`card`)

```json
{
  "headline": "확인해 주세요",
  "title": "메일 보내기",
  "fields": [{"name": "받는 사람", "value": "a@example.com", "inline": true}],
  "body": "안녕하세요, …",
  "footer": "만료 11-19 15:00 · apr-1a2b",
  "tone": "pending",
  "buttons": [
    {"action": "approve:apr-1a2b", "label": "보내기", "primary": true, "prompt": null, "form": null},
    {"action": "reject:apr-1a2b", "label": "거절", "primary": false, "prompt": null, "form": null}
  ]
}
```

| 필드 | 형식 | 설명 |
|---|---|---|
| `headline` | 문자열 | 맨 위 한 줄(알림 미리보기용). 닫힌 카드에서는 결과 문구다(예: "✅ 보냈습니다"). |
| `title` | 문자열 | 제목(고정 문구) |
| `fields` | 배열 | `{name: 고정 문구, value: 마크다운, inline: 옆 칸과 나란히 놓아도 되는 짧은 값}` |
| `body` | 문자열 | 본문(마크다운, 비어 있을 수 있음). 모델이 쓴 글은 이스케이프돼 있다. |
| `footer` | 문자열 \| null | 꼬리말(시각, ID 같은 데몬 값) |
| `tone` | 문자열 | `pending`(대기), `success`, `danger`, `warning`, `neutral`. 색과 아이콘에 쓴다. |
| `buttons` | 배열 | 아래. 닫힌 카드는 보통 비어 있다. |

버튼:

| 필드 | 형식 | 설명 |
|---|---|---|
| `action` | 문자열 | 누를 때 `press.action`으로 그대로 보낸다. 해석하지 않는다. |
| `label` | 문자열 | 버튼 글 |
| `primary` | 불 | 되돌릴 수 없는 일(보내기 등). 강조해 그린다. |
| `prompt` | 객체 \| null | 있으면 누를 때 값을 하나 입력받는다(인증 코드 등). |
| `form` | 객체 \| null | 있으면 누를 때 **여러 칸**을 입력받는다(아이디·비밀번호 등, 결정 0063). 아래. |

`prompt`:

| 필드 | 형식 | 설명 |
|---|---|---|
| `title` | 문자열 | 입력창 제목(45자까지) |
| `label` | 문자열 | 칸 이름(45자까지) |
| `placeholder` | 문자열 \| null | 자리 표시 글 |
| `min_len`, `max_len` | 정수 | 입력 길이 범위. 앱이 먼저 확인한다. |

- 입력값은 `press.input`으로 보낸다. 앱은 이 값을 저장하거나 로그에 남기지 않는다.

`form`(결정 0063):

| 필드 | 형식 | 설명 |
|---|---|---|
| `title` | 문자열 | 입력창 제목(고정 문구) |
| `fields` | 배열 | 입력 칸. 위에서 아래 순서다. |

`form.fields[]`:

| 필드 | 형식 | 설명 |
|---|---|---|
| `key` | 문자열 | 값의 이름. `press.inputs`의 키로 그대로 쓴다. |
| `label` | 문자열 | 칸 이름(고정 문구) |
| `kind` | 문자열 | `text`(보이는 글, 아이디 등), `secret`(가려서 받는 글, 비밀번호), `check`(켜고 끄는 칸) |
| `min_len`, `max_len` | 정수 | 글 칸의 길이 범위(자). 앱이 먼저 확인한다. `check`에는 뜻이 없다(0). |

- **`form`이 있는 버튼은 `welcome.capabilities.forms`가 `true`일 때만 입력창을 연다.** 입력창을 그리지 못하는 앱은 그 버튼을 그리지 않고, 카드 본문의 링크로 안내한다(보통 버튼으로 그리면 눌러도 아무 일이 없다, 5.6). `kind`를 모르는 칸이 하나라도 있으면 같게 한다 [제안].
- 입력값은 `press.inputs`로 보낸다: 모든 `key`에 값을 하나씩. `check` 칸은 불(`true`/`false`)로 보낸다. 앱은 이 값을 저장하거나 로그에 남기지 않는다(`secret`은 특히 13장).
- `form`은 Discord에서는 그려지지 않는다. 앱이 끊겨 Discord DM으로 돌아간 카드에는 이 버튼이 없다(11장).

#### 예: 로그인 입력 카드(`login_input`)

알림 받을 곳이 앱이고 사이트에 로그인이 필요한데 저장된 로그인이 없을 때(또는 저장된 로그인이 사이트에 거절됐을 때) 온다.

```json
{
  "headline": "🔐 **로그인이 필요합니다** 예시 사이트",
  "title": "로그인 정보 입력",
  "fields": [
    {"name": "사이트", "value": "https://example.com", "inline": true},
    {"name": "까닭", "value": "게시판을 읽으려면 로그인이 필요합니다.", "inline": false}
  ],
  "body": "사이트 주소가 맞는지 확인하고 [로그인 정보 입력]을 눌러 아이디와 비밀번호를 넣어 주세요(실패하면 3번까지). 넣은 값은 Lucia(모델)에게 가지 않고 데몬이 브라우저에 바로 넣습니다.\n버튼이 없거나 안 되면: https://<관리 페이지 주소>/site-login/<토큰>",
  "footer": "요청 3f2a9c0e · 11-21 18:40까지 답이 없으면 로그인하지 않습니다",
  "tone": "pending",
  "buttons": [
    {"action": "approval:3f2a9c0e5b7d4c1a8e6f0b2d9a4c7e13:approve", "label": "로그인 정보 입력", "primary": true,
     "prompt": null,
     "form": {"title": "로그인 정보 입력", "fields": [
       {"key": "username", "label": "아이디", "kind": "text", "min_len": 1, "max_len": 100},
       {"key": "password", "label": "비밀번호", "kind": "secret", "min_len": 1, "max_len": 200},
       {"key": "save", "label": "이 로그인 저장(다음부터는 승인만)", "kind": "check", "min_len": 0, "max_len": 0}]}},
    {"action": "approval:3f2a9c0e5b7d4c1a8e6f0b2d9a4c7e13:deny", "label": "취소", "primary": false,
     "prompt": null, "form": null}
  ]
}
```

- 누르는 쪽: 사용자가 입력창에서 확인하면 `{"type": "press", "ref": …, "action": <첫 버튼의 action>, "inputs": {"username": "…", "password": "…", "save": true}}`를 보낸다(5.6).
- 카드는 곧바로 닫힌다(`card_closed`, `headline`이 "⏳ 로그인하고 있습니다. 결과는 곧 알려 드립니다."). 이것은 "받았다"는 뜻이지 로그인이 됐다는 뜻이 아니다. 로그인은 데몬에서 떨어진 작업으로 돌고(결정 0038) 몇 초에서 수십 초가 걸린다.
- **결과는 경우에 따라 다르게 온다. 결과를 기다리는 표시를 붙들어 두지 않는다.**

| 결과 | 앱이 받는 것 |
|---|---|
| 로그인 성공 | "✅ …에 로그인했습니다. …" `text` |
| 이미 로그인돼 있었다 | "✅ <사이트>: 이미 로그인돼 있었습니다. …" `text`(넣은 비밀번호는 확인하지 못해 저장하지 않는다) |
| 인증 코드가 필요하다 | 인증 코드 카드(`prompt`가 있는 카드). `text`는 없다 |
| 사이트가 사람 확인을 띄웠다 | "🧩 …" `text`(화면 넘겨받기 안내, 6.7) |
| 사이트가 거절하는 등 다시 넣을 수 있는 실패 | "⚠️ … 로그인하지 못했습니다: …" `text`와 **새 카드**(새 `ref`, "앞선 시도" 칸에 까닭) |
| 시도를 다 썼다(3번) | "⚠️ … 이 링크로 3번 시도해 닫았습니다. …" `text`만. 카드로 한 시도도 "이 링크"라고 쓰여 온다. 다시 하려면 Lucia에게 요청한다. |
| 시도하지 못했다(빈 칸·너무 긴 값, 사람이 브라우저를 넘겨받는 중) | 새 카드만(`text`는 없고, 시도로 세지 않는다) |
| 150초 안에 끝나지 않았다 | "⚠️ …" `text` + 새 카드(시도로 센다) |

- 시도는 한 요청에 모두 3번이다(카드와 본문의 링크를 합쳐 센다). 링크에서 낸 실패에는, 먼저 올린 카드가 아직 열려 있으면 새 카드를 올리지 않는다(같은 요청의 열린 카드는 하나다).
- 취소하면 `card_closed`("🚫 거절했습니다. 로그인하지 않았습니다.")가 오고, 같은 요청의 링크도 닫힌다. 답이 없어 만료되면 "⌛ 답이 없어 만료됐습니다. …"로 닫힌다(링크는 제 시간에 스스로 닫힌다).
- 이 카드를 누를 때 요청이 이미 닫혔다면(링크에서 먼저 처리했거나, 시간이 지났거나, 시도를 다 썼다면) "❌ …"로 닫힌다.
- 본문의 링크(`/site-login/<토큰>`)는 관리 페이지의 입력 링크와 같은 요청이다. 같은 일을 하는 다른 길이라, 카드에서 하든 링크에서 하든 시도 수는 합쳐 센다.
- 승인 카드는 만료 시간이 있다. 답이 없으면 거부로 닫힌다(`card_closed`, "⌛ 답이 없어 만료됐습니다").

### 9.2 파일

```json
{"id": "f_9c1e0a7b2d4f", "name": "정리.md", "size": 1834, "mime": "text/markdown", "kind": "text"}
```

| `kind` | 뜻 |
|---|---|
| `image` | 이미지 |
| `pdf` | PDF |
| `text` | 텍스트 계열 |
| `office` | Office 문서 |
| `other` | 그 밖(Lucia가 열 수 없는 종류) |

`mime`은 서버가 파일 서명으로 정한 값이다.

기록(`history`, `shape: 2`)의 파일에는 `reachable`(받을 수 있는가)이 늘 더해지고, 지워졌으면 `gone: true`도 붙는다(6.2).

### 9.3 기록 항목

6.2를 본다: `{id: 정수, at: RFC 3339, from: "user"|"lucia"|"daemon", conversation: 문자열|null, body: 문자열}`. `shape: 2`로 읽으면 `parts`(문자열 배열)와 `files`(파일 객체 배열)가 있을 수 있다. 이 필드들은 없을 수 있고, 있을 때만 뜻이 있다.

---

## 10. 푸시(FCM)

### 10.1 준비

- 데몬: 관리 페이지 → 연결 → "앱 알림과 푸시"에 Firebase **서비스 계정 JSON**을 넣는다.
- 앱:
  - 같은 Firebase 프로젝트로 FCM SDK를 설정한다. iOS는 Firebase 콘솔에 APNs 인증 키를 등록한다.
  - 등록 토큰을 받으면, 그리고 바뀔 때마다 `push.register`나 `PUT /v1/push`로 등록한다.

### 10.2 언제 오나

앱 대화의 프레임은 모든 앱의 상자에 들어가지만(11장), **푸시는 울릴 앱으로 골라진 앱에만** 간다(11.1). 아래 표는 울릴 앱으로 골라졌다고 보고 읽는다.

| 상황 | 푸시 |
|---|---|
| 앱이 연결돼 있고 `foreground` | 보내지 않음(프레임으로 울린다) |
| 앱이 연결돼 있고 `background`나 `away` | `text`·`files`·`card`일 때 보냄 |
| 앱이 끊겨 있음(푸시 토큰 있음) | `text`·`files`·`card`일 때 보냄. 프레임은 상자에 쌓인다. |
| `note`, `card_closed`, `sent`, `typing` | 보내지 않음 |
| 이 앱의 프레임에 `alert: false`가 붙음(다른 앱이 울릴 앱이다) | 처음에는 보내지 않음. 넘길 때 보낸다(11.1). |
| 같은 알림을 다른 기기에서 봤음 | 내용 없는 `clear` 푸시(10.4) |

- 글(`text`·`files`)은 한 앱에 **10초에 한 번까지**만 보낸다. 그 사이 것은 앱을 열면 받는다.
- 카드는 울릴 앱에 늘 보낸다.
- 어느 앱을 울릴지 데몬이 고른다(11.1). 그 앱이 푸시를 받는 앱이면 앞에 있지 않을 때 바로 푸시로 깨운다. 어느 앱에서 한 말의 답은 **그 앱을 먼저** 울리므로, 휴대폰에서 묻고 주머니에 넣었으면 데스크톱 창이 앞에 있어도 휴대폰이 푸시로 울린다. 울리지 않는 앱은 프레임을 상자에 받아 두기만 하고 푸시하지 않는다. 푸시를 받는 앱에 푸시가 가는 것은 그 앱이 울릴 앱일 때, 그리고 다른 앱이 울려 놓고 보지 않은 알림을 넘길 때다.

### 10.3 메시지 모양

```json
{
  "notification": {"title": "Lucia", "body": "새 메시지가 있습니다"},
  "data": {"type": "wake", "kind": "text", "seq": "58", "client": "cl-3f9a0c12de"},
  "android": {"priority": "normal", "collapse_key": "lucia-messages",
              "notification": {"tag": "lucia-messages"}},
  "apns": {"headers": {"apns-priority": "5", "apns-collapse-id": "lucia-messages"}}
}
```

| `data.kind` | 기본 본문 | 우선순위 | 접기 |
|---|---|---|---|
| `text` | 새 메시지가 있습니다 | 보통(Android `normal`, APNs 5) | `lucia-messages`(앞 알림을 바꿔 끼움) |
| `files` | 파일이 왔습니다 | 보통 | `lucia-messages` |
| `card` | 확인할 카드가 있습니다 | 높음(Android `high`, APNs 10) | 없음(카드마다 따로) |

- 관리 페이지에서 "푸시에 글 앞부분 싣기"를 켜면 본문이 글 앞 120자로 바뀐다(마스킹을 거친다). 기본은 고정 문구다.
- `data`의 값은 모두 문자열이다. `seq`는 그 프레임의 번호다. `data.type`은 `wake`(깨우기)이거나 `clear`(알림 지우기, 10.4)다.
- 관리 페이지의 "시험 푸시"는 `kind: text`, `seq: "0"`이다.
- 앱 처리:
  - 푸시를 받으면, 또는 사용자가 알림을 누르면 WebSocket으로 연결해 `hello.resume`으로 받는다.
  - **푸시 본문을 대화 내용으로 쓰지 않는다**(미리보기가 꺼져 있으면 고정 문구다).
  - iOS 백그라운드 처리 시간이 모자라면 알림만 보이고, 열 때 받는다.
- 토큰이 무효(UNREGISTERED)가 되면 데몬이 지운다. 그 뒤로는 닿는 앱(연결돼 있거나 푸시를 받을 수 있는 앱)이 하나도 없을 때 글이 Discord DM으로 간다(11장). 앱은 시작할 때마다 토큰을 다시 등록해 두는 것이 안전하다.

### 10.4 알림을 지우는 푸시(`clear`, 결정 0066)

같은 알림을 다른 기기에서 봤으면 **끊겨 있는 앱**에 보낸다. 앱이 띄워 둔 운영체제 알림을 치우게 한다. 연결돼 있는 앱에는 이 푸시 대신 `seen` 프레임이 간다(5.5).

```json
{
  "data": {"type": "clear", "seq": "58", "client": "cl-3f9a0c12de"},
  "android": {"priority": "normal"},
  "apns": {"headers": {"apns-priority": "5"}, "payload": {"aps": {"content-available": 1}}}
}
```

- **데이터만 있다.** `notification`(제목·본문)이 없어 운영체제가 알림을 띄우지 않는다. `collapse_key`도 없다. `data.kind`도 없다.
- `seq`는 **이 앱의 번호**이고, 그 번호까지 봤다는 뜻이다(문자열). 앱은 그 번호 이하의 프레임으로 띄운 알림을 치운다.
- 글 푸시의 10초 간격을 받지 않는다.
- 연결해서 받을 것은 없다. 푸시 처리기가 `data.type`을 보고, `clear`이면 알림만 치우고 끝낸다. `wake`일 때만 움직이는 예전 앱은 이 푸시를 무시한다(알림도 뜨지 않는다).

---

## 11. 메시지가 가는 곳(라우팅)

- **앱 대화는 하나다**(결정 0067). 앱이 몇 개든(휴대폰, 데스크톱) 취소하지 않은 앱은 모두 같은 타임라인을 받는다. 대화 ID(`app:<클라이언트 ID>`)는 앱마다 다르지만 "어느 앱에서 한 말인가"를 가를 뿐이다.
- **앱 대화로 가는 것**:
  - 앱에서 한 말에 대한 답과 진행 안내(어느 앱에서 말했든)
  - 앱 대화가 정한 카드
  - "알림 받을 곳"이 앱이면 브리핑·알림·승인 카드
  - 채팅 처리가 실패했을 때의 안내(아래)
- 앱 대화로 가는 번호 붙는 프레임(`text`·`note`·`files`·`card`·`card_closed`·`sticker`)은 **모든 앱의 상자**에 들어간다. 번호는 앱마다 제 것이다. 입력 중 표시(`typing`)는 연결된 앱 모두에 간다. 사용자가 한 앱에서 한 말은 다른 앱의 상자에 `sent`로 들어간다(5.5).
- "알림 받을 곳"은 관리 페이지 연결 화면에서 **"앱 모두"**(설정 값 `*`)와 **Discord DM**(정하지 않음) 가운데 고른다. 앱 하나를 고르는 것은 없다(앱 대화가 하나라 "이 앱만"은 지킬 수 없다). 예전에 앱 하나로 저장된 값은 "앱 모두"로 읽고, 다음에 저장할 때 `*`로 바뀐다. `welcome.notify_target`은 알림을 앱으로 받으면 모든 앱에 `true`다. 취소하지 않은 앱이 하나도 없으면 Discord DM이다. 정하지 않았으면 앱에서 시킨 일의 승인 카드도 Discord DM으로 간다.
- 앱 대화로 갈 것은 이 순서로 처리된다.
  1. **닿는 앱**(연결돼 있거나, 데몬 자격 증명과 푸시 토큰이 있어 푸시로 깨울 수 있는 앱)이 하나라도 있으면 모든 앱의 상자에 넣는다. 연결된 앱에는 바로 보내고, 어느 앱을 울릴지 데몬이 고른다(11.1). 푸시가 필요하면 상자에 쌓고 푸시로 깨운다. 카드는 앱에서 누른다. 말한 앱이 끊겼고 푸시도 없어도 다른 앱이 닿으면 Discord DM으로 돌리지 않는다.
  2. 닿는 앱이 하나도 없으면 **Discord DM으로 보낸다.** 그 글과 카드는 앱 상자에 들어가지 않는다. 카드는 Discord에서 누른다.
     - 기록(`history`)에는 남는다. 앱 대화에서 한 말의 답은 앱 대화 기록으로 남는다.
     - 값을 여러 칸 받는 버튼(`form`, 9.1)은 Discord에서 그려지지 않는다. 로그인 입력 카드는 본문에 입력 링크를 함께 담아 그 길로 안내한다. Discord로는 비밀번호를 받지 않는다.
- **채팅 처리 오류는 말이 온 곳으로 간다**(결정 0067). 말을 처리하다 실패하면 그 말이 온 대화로 짧은 안내가 간다: `[알림] 답을 만들지 못했습니다: <오류 내용>`.
  - 앱 대화면 위 규칙대로 모든 앱에 간다(물은 앱을 먼저 울린다, 11.1). 데스크톱에서 한 말의 오류가 휴대폰에만 가지 않는다. Discord DM이면 DM으로 간다.
  - 서버 채널(공개된 곳)에는 오류 내용을 싣지 않고 `[알림] 답을 만들지 못했습니다.`만 간다.
  - 같은 오류가 두 번 나면 두 번 알린다(알림 글의 "6시간에 한 번" 거르기를 이 안내에는 걸지 않는다). 오류 내용은 전처럼 알림 기록(관리 페이지 상태 화면)에 남는다.
  - 그 대화로 보내지 못하면 전처럼 알림 받을 곳으로 알린다.
- 소유자가 Discord DM에서 직접 한 대화의 답은 DM으로 간다. DM의 대화는 앱 타임라인에 실시간으로 들어가지 않는다(기록의 `scope: "owner"`로 읽는다, 6.2).
- 작업 스레드(긴 작업의 진행)는 Discord에만 있다. 앱은 진행을 `tasks.list`(6.9)로 읽는다.

### 11.1 어디에 두고 어디를 울리나(결정 0066·0067)

앱이 여럿이면 앱 대화의 프레임을 **어디에 두는가**(모든 앱의 상자)와 **어디를 울리는가**(한 곳)가 나뉜다. 사람이 있는 기기에서 울리고, 보지 않으면 푸시를 받는 앱(휴대폰)으로 넘긴다. 앱이 하나뿐이면 그 앱이 늘 울릴 앱이라 전과 같다.

#### 두는 곳

- 앱 대화(`app:<클라이언트 ID>`나 `app:*`)로 가는 번호 붙는 프레임은 취소하지 않은 앱마다 상자에 넣는다(번호는 앱마다 제 것이다).
- **받는 앱의 대화가 아니면 `conversation`이 붙는다**(5.5): 그 프레임이 속한 대화의 ID다. 다른 앱에서 한 말의 답과 그 대화의 카드에 붙는다. 앱 모두의 대화(`app:*`)에는 붙지 않는다.
- 기록(`history`)에는 한 번만 남는다(말한 대화의 ID로). 앱의 `history`는 모든 앱 대화를 함께 읽는다(6.2).
- 카드 `ref`는 `app:<클라이언트 ID>:<12자>`나 `app:*:<12자>`이고 앱이 달라도 같다. **어느 앱에서 눌러도 같은 카드다**(5.6의 `press`). 닫히면 모든 앱에 `card_closed`가 간다.
- 입력 중 표시(`typing`)는 연결된 앱 모두에 간다.
- 닿는 앱이 하나도 없으면 Discord DM으로 간다(11장). 그때 글과 카드는 앱 상자에 들어가지 않는다.

#### 울릴 곳

울리지 않는 앱의 프레임에는 `"alert": false`가 붙는다(5.4). 데몬이 위에서부터 처음 맞는 줄로 고른다.

| 순서 | 조건 | 울리는 곳 |
|---|---|---|
| ⓪ | 그 프레임이 한 앱(`app:<A>`)의 대화로 가는 것이다(그 앱에서 한 말의 답, 그 대화의 카드) | **A를 먼저.** 아래 ⓪의 규칙 |
| ① | 앞에 있는(`foreground`) 연결이 있다 | 그 가운데 가장 최근에 앞에 온 앱 하나 |
| ② | ①이 없고, 연결돼 있고 `background`이며 푸시 토큰이 없는 앱이 있다 | 그 앱들("스스로 알리는 앱", 푸시가 없는 데스크톱) |
| ③ | ①②가 없고, 푸시를 받을 수 있는 앱이 있다 | 그 앱들(푸시로) |
| ④ | 그 밖 | 연결된 앱 모두(`away`여도 돌아오면 본다) |

⓪(물은 앱 먼저): 한 앱(`app:<A>`)의 대화로 가는 프레임은 **A를 먼저 울린다.**

- A가 앞에 있으면 프레임으로 울린다.
- A가 푸시를 받는 앱이면 앞에 없을 때 바로 푸시로 울린다. 휴대폰에서 묻고 주머니에 넣었으면 데스크톱 창이 앞에 있어도 휴대폰이 울린다.
- A가 연결돼 있고 `background`인 푸시 없는 앱이면 A를 울리고, 때가 되면 넘긴다(아래 "넘기기").
- A가 `away`이거나 끊긴 푸시 없는 앱이면 ①~④로 고른다.

①~④는 그 밖의 프레임에 쓴다: `app:*`의 브리핑·알림·카드, 그리고 ⓪에서 A가 자리에 없을 때.

- "스스로 알리는 앱"과 "푸시가 있어야 알리는 앱"은 **푸시 토큰이 있는가**로 가른다(`hello`에 표시를 더하지 않았다).
- `away`인 앱은 ②에서 빠진다(사람이 없다).
- 울리는 앱에는 평소 규칙대로 간다: 연결돼 있고 `foreground`이면 프레임으로, 아니면(끊김, `background`, `away`) 푸시로(10.2).
- `text`·`files`·`card`·`sticker`만 이 규칙을 탄다. `note`·`typing`·`card_closed`·`sent`는 울리지 않고 `alert` 필드도 없다.

#### 넘기기

울린 앱의 사용자가 그 알림을 안 봤으면, **울리지 않은 푸시를 받는 앱**(휴대폰)으로 넘긴다. 넘기는 방법은 그 앱의 상자에 이미 있는 프레임을 지금 알리는 것이다: 연결돼 있으면 `alert{seq}` 프레임(5.5)을 보내고, 앞에 있지 않으면(끊김, `background`, `away`) 푸시도 보낸다. 다른 앱에서 한 말의 답과 그 대화의 카드도 같다. 프레임은 모든 앱의 상자에 이미 있으므로 넘기기는 늘 `alert{seq}`다(프레임을 복사해 넣는 일은 없다).

- **때가 되면**: 글(`text`·`files`·`sticker`)은 **2분**, 카드는 **20초** 안에 그 알림을 봤다는 `seen`이 없으면 넘긴다 [제안]. 다만 **울린 앱이 `seen`을 말하는 앱이거나 푸시가 없는 앱(데스크톱)일 때만**이다. `seen`을 말하는 앱은 이 데몬이 뜬 뒤 `seen`이나 `presence: away`를 한 번이라도 보낸 앱이다. 앞에 있는 앱도 같은 규칙이다.
  - 푸시가 없는 앱은 `seen`을 말하지 않아도 넘긴다(결정 0067). 그 앱에서 이미 봤으면 휴대폰이 늦게 한 번 더 울리지만, 승인 카드가 아무 데도 닿지 않고 만료되는 것보다 낫다. 데스크톱 앱이 `seen`을 보내면 이 두 번째 울림이 없어진다.
- **자리를 뜨면 바로**: 울린 앱이 `away`를 보내거나 연결이 끊기면 기다리지 않고 넘긴다.
  - `away`를 보낸 앱은 `seen`을 말하는 앱으로 친다(데몬이 다시 뜬 뒤 아직 `seen`이 없어도 카드가 그대로 만료되지 않게).
  - 다만 끊긴 앱이 푸시를 받는 앱(휴대폰)이고 `seen`도 `away`도 보낸 적이 없으면 넘기지 않는다. 휴대폰은 뒤로 갈 때마다 끊기기 때문이다.
- **`seen`을 말하지 않는 푸시를 받는 앱(휴대폰)이 울린 것은 때를 기다려 넘기지 않는다.** 예전 휴대폰 앱은 `seen`을 보내지 않는다. 그런 앱이 울린 것까지 넘기면 봤는지 알 길이 없어 같은 알림이 두 번 울린다.
- 울릴 앱이 ③이나 ④로 골라졌으면 넘기지 않는다(③은 푸시를 받는 앱이 이미 울렸고, ④는 연결된 앱 모두가 울렸다). ⓪에서 푸시를 받는 앱 A가 앞에 없어 푸시로 울렸을 때도 넘기지 않는다.
- 카드의 만료가 가까울 때 넘기기를 앞당기거나, 급한 것을 처음부터 모두 울리는 것은 하지 않는다(카드에 만료가 실리지 않고, 급한지를 가를 표지가 없다). 카드는 20초로 충분히 짧다.
- 데몬의 장부는 메모리에만 있다. 데몬이 다시 시작하면 기다리던 넘기기와 "`seen`을 말하는 앱"의 기억이 사라진다(상자의 프레임은 남는다). 그래서 앱은 연결할 때마다 `seen`을 한 번 보낸다(12.6).

#### 봤음

- 앱 → 데몬 `seen{seq}`(5.6): 그 번호까지 모두 봤다. `press`와 `message`도 그 앞을 본 것으로 친다(`seen`을 말하지 않는 앱도).
- 데몬은 같은 알림을 가진 다른 앱에 알린다: 연결돼 있으면 `seen{seq}`(받는 앱의 번호, 5.5), 끊겨 있으면 `clear` 푸시(10.4). 받은 앱은 그 알림을 치운다. 본 알림은 넘기기도 멈춘다.
- 알리는 번호가 하나라서, 그 앱에 **보지 않은 알림이 앞에 있으면** 그 앞까지만 알린다(다른 곳에서 본 알림의 번호에서만). 앞의 것을 본 뒤에 나머지가 간다.

#### 예전 앱과의 호환

- `sent`는 모르는 `type`이라 예전 앱이 무시한다(4장). 그런 앱에는 다른 기기에서 한 말의 **답만** 보이고 물음은 안 보인다. `history`를 다시 읽으면 보인다.
- `alert: false`, `seen`, `alert`, `conversation`도 모르는 앱이 무시한다. 다만 **`alert: false`를 모르는 앱은 그 프레임을 평소처럼 울린다.** 푸시가 없는 데스크톱 앱이 `alert: false`를 모르면 다른 기기 대화의 답이 올 때도 울린다. 앱이 `alert: false`·`alert`·`seen`을 지키도록 고쳐야 같은 알림이 한 번만 울린다(12.6, 12.8).
- 지금의 휴대폰 앱(Lucent)은 코드를 읽어 본 바로는 이렇게 움직인다. 번호가 건너뛰어도 잃은 것으로 보지 않는다(`sent`를 버려도 괜찮다). 푸시를 등록해 두었으면 실시간 프레임으로 스스로 알림을 띄우지 않고 서버 푸시에 맡기므로 `alert: false` 프레임 때문에 두 번 울리지는 않는다. 다만 넘겨받은 알림을 `alert{seq}`로 받아도 알림을 띄우지 않고(모르는 `type`), `seen`·`clear`로 알림을 치우지 않는다. 데스크톱에서 본 것의 휴대폰 알림이 남는다.
- 푸시의 `clear`는 `data.type`이 `wake`일 때만 움직이는 예전 앱이 무시한다(10.4).
- `seen`을 보내지 않는 휴대폰 앱이 울린 알림은 때를 기다려 넘기지 않는다. 그 앱도 누르거나 말하면 본 것으로 친다.
- 그래서 대화가 온전히 하나로 보이려면 휴대폰 앱과 데스크톱 앱 모두 12.6·12.8을 따라야 한다. 데몬만 바뀌어도 대화의 답·알림·오류가 모든 앱에 가는 것은 바로 된다.

---

## 12. 앱 구현 지침

### 12.1 앱이 저장할 것

| 값 | 설명 |
|---|---|
| 토큰 | 안전한 저장소에 둔다 |
| 서버 주소 | 집 안, 밖 |
| `lastSeq` | 처리를 마친 마지막 `seq`. 처음은 0. |
| 화면용 기록 | 받은 프레임이나 `history` 결과. 카드는 `ref`로 찾아 바꿔 그린다. |
| Lucia의 프로필 | `profile.name`과 `profile.avatar.id`, 그 그림 파일(8.8). ID가 같으면 그림을 다시 받지 않는다. 연결하지 못해도 마지막으로 받은 이름과 그림을 보인다. |

### 12.2 프레임 처리

1. `seq ≤ lastSeq`이면 버린다(중복).
2. 화면에 반영하고 기기에 저장한다.
3. `lastSeq = seq`.
4. `ack`를 보낸다. 매번 보내도 되고, 1초쯤 모아 마지막 값 하나만 보내도 된다.

### 12.3 연결 관리

- 시작할 때, 앞으로 올 때, 푸시를 받았을 때 연결한다. `hello.resume = lastSeq`(처음이면 0)를 준다.
- `welcome` 뒤 `resumed.truncated`가 `true`이면 `history`를 다시 읽고 `lastSeq = resumed.last`로 맞춘다.
- 처음 설치했으면 `history`로 화면을 채운다. `scope: "app"`은 모든 앱 대화(이 앱의 타임라인 전체, 12.8)이고, `"owner"`면 Discord에서 나눈 대화까지다.
- 끊기면 지수 백오프로 다시 연결한다. 1초에서 시작해 두 배씩, 30초까지 늘리고 흔들림을 섞는다.
  - `unauthorized`나 `unsupported_role`로 끊겼으면 다시 연결하지 않는다. 사용자에게 토큰을 다시 넣게 한다.
  - `too_many_attempts`이면 10분 기다린다.
- 뒤로 갈 때는 `presence: background`를 보낸다. OS가 소켓을 닫아도 괜찮다(푸시가 깨운다). 앞으로 오면 `presence: foreground`를 보내거나 새로 연결한다.
- 사용자가 기기 앞에 없을 때(데스크톱에서 오래 손대지 않았거나 화면을 잠갔을 때 등)는 `presence: away`를 보낸다(`capabilities.presence_away`가 있을 때). `background`는 "앱이 뒤에 있다"이고 `away`는 "사람이 없다"다.
- 새 연결은 `foreground`·책상 상태 꺼짐으로 시작한다. 다시 붙으면 `welcome` 뒤에 지금 자리(`background`·`away`), 책상 상태(`desk`, 켜져 있으면), 마지막으로 본 번호(`seen`, 12.6)를 보낸다.
- 말을 보낼 때는 `message.id`를 붙인다. `accepted`가 오지 않고 끊기면, 다시 연결한 뒤 `history`로 그 말이 들어갔는지 보고 다시 보낼지 정한다.
  - 데몬은 같은 말을 거르지 않는다. 두 번 보내면 두 번 답한다.

### 12.4 파일 보내기

1. `POST /v1/files?name=…`로 올려 `id`를 받는다.
2. `message.files`에 넣어 보낸다(글은 비워도 된다).
3. 올린 뒤 오래 붙이지 않은 파일은 30일 뒤 지워진다.

### 12.5 카드

- `card`는 `ref`로 저장한다. `card_closed`가 오면 같은 `ref`를 바꿔 그린다.
- 버튼을 누르면 결과(`card_closed`)가 올 때까지 그 카드의 버튼을 잠근다. 잠깐 기다려도 결과가 없으면 다시 누를 수 있게 푼다.
- 카드 내용(받는 사람, 본문 등)은 사용자가 무엇을 승인하는지 알 수 있게 줄이지 말고 보인다.
- **입력 칸이 있는 버튼(`form`, 결정 0063)**:
  - `capabilities.forms`가 `true`일 때만 입력창을 연다. 못 그리면 그 버튼을 그리지 않고 본문의 링크로 안내한다.
  - `kind`대로 그린다: `text`는 보이는 글 칸, `secret`은 가려서 받는 칸(입력 종류를 비밀번호로), `check`는 켜고 끄는 칸. 칸 이름은 `label`이다. 칸마다 `min_len`~`max_len`을 보내기 전에 확인한다.
  - `check` 칸의 처음 값은 정하지 않아 앱이 정한다. 관리 페이지의 입력 링크는 "저장해 두기"를 켠 채로 시작한다 [제안: 같게 한다]. 저장을 끈 채 로그인에 성공하면 그 사이트에 이미 저장돼 있던 로그인이 지워진다.
  - 보낼 때는 `press.inputs`에 모든 `key`의 값을 담는다(`check`는 불). **글 값은 다듬지 않고 보낸다.**
  - 보낸 뒤에는 `card_closed`가 올 때까지 그 카드의 버튼을 잠근다. 입력창의 값은 보낸 즉시 지운다(저장·로그·분석·오류 보고에 남기지 않는다, 13장).
  - 카드가 닫혀도 로그인은 끝난 것이 아니다. 닫힌 카드의 "⏳ …"는 "받았다"는 뜻이다. 결과는 뒤에 `text` 알림이나 새 카드로 온다(9.1의 표, 길면 150초까지).

### 12.6 `seen`과 `alert`(결정 0066)

앱이 여럿일 때(11.1) 휴대폰과 데스크톱이 같은 알림을 한 번만 울리게 하려면, 앱이 사용자가 **봤다**고 알려야 한다. **휴대폰 앱도 보낸다.** 보내지 않으면 휴대폰에서 본 것이 데스크톱에 남는다. 앱 대화의 모든 프레임(다른 기기에서 한 말의 답, 승인 카드, 브리핑)이 이 규칙을 탄다(결정 0067).

- `capabilities.seen`이 있으면 사용자가 알림을 봤을 때 `seen{seq}`를 보낸다. 예: 대화 화면이 앞에 있을 때, 사용자가 푸시 알림을 눌러 앱을 열었을 때, (데스크톱) 말풍선 위에서 마우스를 움직였을 때.
- **모자라게 알린다.** 잘못 알린 `seen`은 휴대폰으로 넘어갈 승인 카드를 막고, 알리지 못한 것은 휴대폰이 한 번 더 울릴 뿐이다. 확실하지 않으면 보내지 않는다.
- 글은 스쳐 봐도 되지만 **카드는 건드리거나 대화를 열어야** 본 것으로 친다(`press`는 데몬이 알아서 친다).
- `seq`는 번호 하나다. 그 번호까지 **모두** 봤을 때만 보낸다. 보지 않은 카드가 있으면 그 뒤에 온 글도 알릴 수 없다(그 글은 넘어간다).
- **연결할 때마다** `welcome` 뒤에 지금까지 본 마지막 번호를 한 번 보낸다. 본 것이 없으면 `seq: 0`을 보낸다. 데몬이 다시 시작하면 "`seen`을 말하는 앱"의 기억이 사라지기 때문이다. 끊긴 동안 본 것도 이때 알린다.
- 프로그램을 켜기 전에 온 것은 봤다고도 안 봤다고도 알리지 않는다.
- 데몬의 `seen{seq}`를 받으면 그 번호까지의 알림(말풍선, 운영체제 알림, 깜박임)을 치운다. 더 새로 온 것이 남아 있으면 그것은 둔다. 되돌려 `seen`을 보내지 않는다.
- `alert: false` 프레임은 대화 화면에 놓되 말풍선·운영체제 알림·소리·깜박임을 내지 않는다.
- `alert{seq}`를 받으면 그 프레임을 지금 알린다. 이미 봤으면 아무 일도 없다. 프로그램을 다시 켠 뒤에는 어느 프레임인지 모를 수 있다. 그때는 아무것도 하지 않는다.
- 푸시 처리기는 `data.type: "clear"`를 받으면 그 `seq`까지의 알림만 치우고 끝낸다(연결하지 않는다, 10.4).
- 데스크톱 앱은 `away`(5.6)도 보낸다. 그 순간 그 앱이 울려 놓고 보지 않은 알림이 휴대폰으로 넘어간다.

### 12.7 메모와 맡은 일 맞추기(결정 0066)

- `welcome.methods`에 `notes.list`가 있을 때만 메모를 데몬과 맞춘다. 연결할 때와 `notes` 프레임이 올 때 `notes.list`의 `since`로 읽고, 기기에서 고친 것은 잠깐(1초쯤) 모았다가 `notes.put`으로 올린다. `base`는 그 메모에 대해 마지막으로 받은 `updated_at`이다(6.8).
- **고치는 중인 메모는 올리지도 덮지도 않는다.** 글자마다 저장하는 앱이 치는 도중에 맞추면 글이 갈라진다. 다 고치면(초점이 떠나면) 올린다.
- `tasks.list`는 연결할 때와 `tasks` 프레임마다 읽는다(6.9). 끝나지 않은 것을 "맡은 일"로 보이면 된다.

### 12.8 한 대화 맞추기(결정 0067)

`welcome.capabilities.shared_conversation`이 `true`이면 데몬이 앱을 기기가 여럿이어도 한 대화로 다룬다. 앱은 이렇게 한다.

- **`sent`**(5.5)는 사용자가 **다른 기기**에서 한 말이다. 사용자 자신의 말풍선으로 그린다. 어느 기기에서 한 말인지 `from.name`을 작게 붙여도 된다. `files`와 `sticker`는 그 말풍선에 붙이고, `context_note`는 고른 글 표지로 보인다. 보낸 앱은 `sent`를 받지 않으므로 자기가 한 말은 보내는 즉시 제 화면에 그린다.
- **`alert: false`·`alert`·`seen`·`clear`를 지킨다**(12.6). 앱 대화의 모든 프레임이 이 규칙을 탄다. 지키지 않으면 다른 기기에서 한 말의 답이 올 때마다 이 기기도 울린다.
- 번호 붙는 프레임은 `conversation`이 있어도 **같은 타임라인에 도착한 차례로** 놓는다. `conversation`이 이 앱의 대화가 아니면 다른 기기에서 한 말의 답(이나 그 대화의 카드)이다. 카드는 어느 앱의 것이든 `ref`로 누를 수 있다.
- 실시간 프레임과 `history`는 서로의 ID를 모른다(프레임에는 기록 ID가, 기록에는 `seq`가 없다). 겹쳐 그리지 않으려면 시각으로 맞춘다. `sent`에는 `at`이 있다. 다만 `sent.at`은 데몬이 말을 **받은** 시각이고 기록의 `at`은 데몬이 그 말을 **처리하기 시작한** 시각이라, 앞의 일을 하는 중이면 기록의 `at`이 `sent.at`보다 늦을 수 있다(기다린 만큼). 시각이 같기를 바라지 말고, 가까운 시각에 같은 대화(`conversation`)·같은 글인 것끼리 맞춘다(기록의 `body`에는 설명 줄이 더 붙을 수 있다, 5.5).
- `history`의 `scope: "app"`은 모든 앱 대화를 읽으므로 이 앱이 보일 **타임라인 전체**다. 처음 설치했을 때와 `resumed.truncated`가 `true`일 때 이것으로 채운다. `from: "user"` 항목의 `conversation`이 이 앱의 대화가 아니면 다른 기기에서 한 말이다. `"owner"`는 Discord DM을 더한다.
- `typing`은 어느 앱에서 한 말이든 연결된 앱 모두에 온다. 이 앱에서 말하지 않았는데 오면 다른 기기에서 한 말을 처리하는 중이다.
- 다른 앱 대화의 파일은 `owner_files` 범위 없이 받는다(8.5). 기록의 `files[].reachable`을 보고 받을지 정한다.
- `sent`를 아직 그리지 않는 앱도 그 번호는 올리고 `ack`한다(5.4, 12.2). 번호가 건너뛰어도 잃은 것으로 보지 않는다.

**앱이 고치기 전에 사용자에게 보이는 것**: 데몬만 바뀌어도 대화의 답·알림·승인 카드·채팅 오류가 모든 앱에 가는 것은 바로 된다. 앱이 12.6·12.8을 따르기 전에는 이렇다.

| 앱이 하지 않는 것 | 사용자에게 보이는 것 |
|---|---|
| `sent`를 처리 | 다른 기기에서 한 말의 **답만** 보이고 물음은 안 보인다. `history`를 다시 읽으면 보인다. |
| `alert: false`를 지킴 | 푸시가 없는 앱(데스크톱 등)이 스스로 알림을 띄우면 다른 기기 대화의 답이 올 때도 울린다. 푸시를 등록해 서버 푸시에만 맡기는 앱은 두 번 울리지 않는다. |
| `alert`·`seen`·`clear`를 처리 | 넘겨받은 알림이 울리지 않고, 다른 기기에서 본 알림이 이 기기에 남는다(예: 데스크톱에서 본 것의 휴대폰 알림). |

---

## 13. 보안 요구 사항

- 토큰은 안전한 저장소에만 둔다. 로그, 분석, 오류 보고, 백업에 넣지 않는다.
- 밖에서는 `wss`/`https`만 쓴다. 인증서 검증을 끄지 않는다.
- `X-Forwarded-For`·`Forwarded` 머리를 보내지 않는다(2.2).
- 카드의 값 입력(`prompt`, 인증 코드 등)은 저장하거나 로그에 남기지 않는다.
- 입력 칸이 있는 카드(`form`, 결정 0063)의 값, 특히 `secret` 칸(비밀번호)은 `press.inputs`로 보내는 것 말고 어디에도 두지 않는다: 저장, 로그, 분석·오류 보고, 자동 완성 기록에 남기지 않고, 보낸 즉시 메모리에서 지운다. 데몬은 그 값을 Lucia(모델), 로그, 원장, 활동 기록, 앱으로 보내는 프레임 상자, 대화 기록에 넣지 않는다.
- 집 안 주소(`ws`)로 보내는 값은 평문이다(2.3). 밖에서는 반드시 `wss`다.
- 메시지와 카드의 마크다운을 HTML로 해석하지 않는다. 웹뷰로 그린다면 이스케이프한다.
- 앱을 잃어버렸거나 토큰이 샜으면 관리 페이지에서 바로 취소한다.
  - "알림 받을 곳"이 앱이면, 토큰을 가진 사람은 승인 카드를 받아 누를 수 있다. 앱 대화는 하나라서(결정 0067) 어느 앱 토큰이든 앱 대화의 카드를 누르고, 모든 앱 대화(다른 앱에서 한 말과 답)를 읽는다.
  - `security_mode` 범위가 있으면 권한 모드도 바꿀 수 있다.
  - **`owner_files` 범위가 있으면 Discord DM에서 받은 사진·문서를 모두 내려받을 수 있다**(결정 0066). `owner` 기록을 읽는 토큰은 이미 글을 다 보지만, 파일은 무게가 다르다. 그래서 기본이 꺼짐이고 토큰마다 사용자가 켠다. 토큰이 샜다면 바로 취소한다. 서버 채널 대화의 파일은 범위가 있어도 받을 수 없다.
  - **앱 토큰이 새면 다른 앱 대화의 파일도 받을 수 있다**(결정 0067). 다른 앱에서 올린 사진·문서도 받는다(그 앱이 짧게 둔 파일은 뺀다). 같은 토큰으로 그 글은 이미 모두 읽을 수 있고, 대화를 하나로 하는 값으로 받아들였다. 토큰이 샜다면 바로 취소한다.
  - **`browser_handover` 범위가 있으면 로그인된 브라우저 세션을 보고 조작할 수 있다**(결정 0063). 관리 페이지를 밖에서 열려면 2단계 인증이 필요하지만 앱 토큰에는 두 번째 요소가 없다. 그래서 이 범위는 기본이 꺼짐이고 토큰마다 사용자가 켠다. 토큰이 샜다면 **바로 취소한다.** 이미 시작된 넘겨받기는 최대 시간(15분)이 지나거나 관리 페이지에서 "끝내기"를 누를 때까지 남지만, 취소한 토큰은 더 보거나 조작하지 못한다.
  - 주 모델은 범위 없이 바꿀 수 있다(더 큰 모델로 바꾸면 사용량을 빨리 쓴다). 바뀌면 알림 대화에 남는다.
- 넘겨받은 브라우저의 화면 이미지(8.9)는 디스크에 캐시하거나 스크린샷·기록에 남기지 않는다. 화면을 떠나면 버리고 `browser.handover.end`를 부른다. 화면에는 사용자의 계정 정보가 보일 수 있다.
- `event`와 `invoke` 답으로 보내는 외부 글(다른 앱의 알림 등)은 Lucia가 외부 자료로 다룬다. 다만 개인 정보이므로 보낼 것을 사용자가 고르게 한다.
- **화면 사본 같은 파일은 `keep=short`로 올린다**(8.4). 1시간 뒤 지워지고 Lucia의 최근 파일 목록·보관함·답 첨부에서 빠진다. 앱도 그 사본을 오래 저장하지 않는다. 화면에는 무엇이든 보일 수 있다.
- **`message.context`의 고른 글은 남의 글일 수 있다**(웹 페이지에서 고른 것 등). 데몬은 이 글을 외부 자료로만 다루고 기록에 글을 남기지 않는다(`[고른 글 N자 함께 보냄]` 표지만 남는다). 앱은 사용자가 고르지 않은 글을 싣지 않고, `source`(프로그램 이름, 창 제목)도 사용자가 내주기로 한 경우에만 싣는다.
- **`desk` 프레임은 허락이 아니다.** 화면 찍기, 클립보드, 입력 넣기 같은 기능은 `invoke`가 올 때마다 앱이 그때그때 사용자 설정을 보고 정한다. 사람이 없을 때 부르면 안 되는 읽기 기능(화면, 클립보드, 앞 창의 제목, 파일 내용)은 `when: "here"`나 `"attended"`로 내준다(7.2).
---

## 14. 한도와 시간 모음

| 항목 | 값 |
|---|---|
| WebSocket 프레임(앱 → 데몬) | 64 KiB |
| `hello` 기다림 | 10초 |
| 데몬 핑 간격 / 무응답 한도 | 30초 / 75초 |
| `message.text` | 8000자 |
| `text`·`note` 프레임 하나(데몬 → 앱) | 8000자(넘으면 나뉨) |
| 말 하나의 파일 수 | 10 |
| 파일 하나 | 31,457,280바이트(30 MiB) |
| 파일 보관 | 30일 |
| 다시 받기 한 번 | 500 프레임 |
| 상자 보관 | ack한 것 7일, 안 한 것 30일 |
| `history.limit` | 1~100(기본 50) |
| `event.text` / `event.key` | 1000자 / 120자 |
| 이벤트 | 연결마다 한 시간 60개 |
| `hello.features` | 32개, 이름 40자 |
| 명령 답 기다림 | 5초 |
| `session.compact` 답 기다림 | 실행 시간 제한 × 2 + 30초(운영 설정으로 630초). 정리 자체는 보통 20~75초 |
| `press.inputs` | 칸 8개, 칸 이름 1~40자, 값 1000자 |
| `message.context.selection` | 4000자 [제안]. `source.app` 100자, `source.title` 200자 |
| 앱 기능의 답 기다림(`tools[].timeout`, `invoke.timeout`) | 기본 40초, 1~120초 [제안] |
| Lucia의 `app_call`·`app_result` 한 번 기다림 | 45초(그 뒤는 `app_result`로 이어 본다. 데몬은 기능의 `timeout`까지 기다린다) |
| `result.files` | 4개 [제안] |
| `result.files`의 그림이 Lucia에게 그림으로 보이는 한도 | 3장, 합계 3 MB(넘는 것은 ID와 `file_view` 안내) |
| 짧게 두는 파일(`keep=short`) | 1시간 [제안] |
| 메모 | 40장(뗀 것은 빼고), 글 1~2000자, 뗀 메모(묘비) 30일 |
| 맡은 일(`tasks.list`) | 30개, 끝난 일은 24시간 [제안]. `tasks` 프레임은 10초에 한 번까지 [제안] |
| 알림 넘기기(11.1) | 글 2분, 카드 20초 [제안] |
| 로그인 입력 카드의 칸 | 아이디 1~100자, 비밀번호 1~200자(카드의 `form`이 알린다) |
| 로그인 입력 카드·링크 | 10분(설정 `[browser] login_input_minutes`, 실패하면 다시 10분). 한 요청에 시도 3번(카드와 링크 합쳐), 로그인 하나에 150초 |
| 브라우저 화면 넘겨받기 | 한 번에 최대 900초(15분) |
| `browser.handover.input` | `text` 1~2000자, `dy` ±3000, `key` 여섯 가지(`enter`, `tab`, `backspace`, `escape`, `up`, `down`) |
| 인증 실패 | 한 주소 10분에 10번이면 그 10분 동안 막음 |
| 클라이언트 수 | 20 |
| 글 푸시 간격 | 한 앱 10초 |
| 푸시 미리보기 | 120자 |
| 푸시 토큰 | 4096자, `[A-Za-z0-9:_.-]` |
| 프로필 그림 | PNG·JPEG(움직이지 않음), 정사각형, 한 변 128~2048px, 3,145,728바이트(3 MiB)까지 |
| 프로필 이름 | 1~24자 한 줄(결정 0064) |

`[제안]`인 값(보관 기간, 이벤트 상한, 푸시 간격 등)은 바뀔 수 있다. 바뀌면 이 표를 고친다.

---

## 15. 오류 코드 모음

### 15.1 WebSocket `error` 프레임

| code | 경우 | 연결 |
|---|---|---|
| `hello_required` | 첫 프레임이 `hello`가 아니다 | 끊음 |
| `unauthorized` | 토큰이 틀렸거나 취소됐다(인증 때, 또는 연결 중 취소) | 끊음 |
| `too_many_attempts` | 그 주소가 인증 실패로 막혀 있다 | 끊음 |
| `unsupported_role` | 받지 않는 역할 | 끊음 |
| `bad_frame` | 깨진 JSON, 모르는 `type`, 바이너리 프레임, 두 번째 `hello`, 틀린 `presence.state`(`foreground`·`background`·`away` 밖), `press.inputs` 한도 초과(칸 8개·칸 이름 40자·값 1000자)나 모양이 틀림, 모양이 틀린 `seen`·`desk`·`message.context`(`seq`가 정수가 아니다 등) | 유지 |
| `empty` | 빈 말(글·파일 없음. `context`만 있는 것도), 빈 이벤트 | 유지 |
| `too_long` | 말이 8000자를 넘는다 | 유지 |
| `bad_file` | 쓸 수 없는 파일 ID, 파일 10개 초과, 파일을 받지 않는 데몬 | 유지 |
| `bad_ref` | 앱 카드가 아닌 `press`(`app:`으로 시작하지 않는 `ref`. 어느 앱의 카드든 누를 수 있다, 결정 0067), 기다리는 부탁이 아닌 `result`(시간이 지나 데몬이 버린 부탁의 늦은 답 포함) | 유지 |
| `too_many_events` | 이벤트 한 시간 상한 | 유지 |
| `unsupported` | 이벤트 입구가 없는 데몬 | 유지 |
| `forbidden` | (기기 연결) 이벤트가 아닌 프레임 | 유지 |
| `closed` | 데몬이 멈추는 중 | 곧 끊김 |

### 15.2 메서드 `result.error.code`

| code | 경우 |
|---|---|
| `unknown_method` | 없는 메서드 |
| `forbidden` | 필요한 범위가 없다(`security_mode`, `browser_handover`) |
| `bad_params` | `params`가 틀렸다(`history`의 `scope`·`shape`, `notes.*`의 인자·`id` 모양·글 길이·40장 초과 등) |
| `unknown_command` | 없는 명령 |
| `timeout` | 처리기가 제때 답하지 않았다 |
| `busy` | 같은 일이 이미 돌고 있다(`session.compact`) |
| `not_active` | 넘겨받은 상태가 아니다(`browser.handover.input`, 6.7) |
| `unavailable` | 브라우저를 쓸 수 없다(`browser.handover.*`, 6.7. WebSocket에서는 `unknown_method`가 먼저 온다) |
| `failed` | 처리하지 못했다(저장소 오류, 브라우저 넘겨받기 시작·입력 실패, 맡은 일 목록을 만들지 못함 등) |
| `unsupported` | 이 데몬이 하지 않는 일 |
| `closed` | 데몬이 멈추는 중 |

`notes.put`의 충돌은 오류가 아니라 `conflict: true`인 답이다(6.8).

### 15.3 HTTP

| 상태 | code | 경우 |
|---|---|---|
| 400 | `forbidden` | 출처를 알 수 없다(프록시 머리 문제) |
| 400 | `bad_params` | 기록·푸시·올리기 매개변수가 틀렸다(`history`의 `scope`·`shape`, `files`의 `keep`). `shape`가 정수가 아니면 프레임워크의 `400`이고 본문 형식이 다르다. |
| 400 | `bad_file` | 올린 파일이 비었거나 너무 크다 |
| 401 | `unauthorized` | 토큰 문제 |
| 403 | `forbidden` | 받지 않는 출처, 기기 토큰, `browser_handover` 범위가 없는 토큰(8.9) |
| 404 | `not_found` | 없는 파일, 이 토큰이 받을 수 없는 파일(`owner_files` 범위가 없는 Discord DM의 파일, 다른 앱이 짧게 둔 파일), 시각이 지난 짧게 둔 파일, 없는 넘겨받기 탭(8.9) |
| 404 | `unavailable` | 브라우저가 꺼진 데몬(8.9) |
| 409 | `not_active` | 넘겨받은 상태가 아니다(8.9) |
| 410 | `gone` | 보관 기간이 지난 파일 |
| 413 | `too_large` | 올린 본문이 상한을 넘었거나 끊겼다 |
| 415, 422 | (프레임워크) | `PUT /v1/push`의 JSON 문제 |
| 429 | `too_many_attempts` | 그 주소가 막혀 있다 |
| 501 | `unsupported` | 파일을 받지 않는 데몬, `POST /v1/voice`(늘, 8.10) |

### 15.4 앱이 `invoke`에 답하는 오류와 Lucia가 받는 오류

- 앱이 `result`의 `error.code`로 답하는 코드는 앱이 정한다. 권장: `denied`, `timeout`, `unavailable`, `bad_params`, `failed`(7.1).
- 데몬이 Lucia에게 돌려주는 오류(`offline`: 앱이 연결돼 있지 않다, `unsupported`: 그 기능을 내주는 앱이 없다, `forbidden`: `when`이나 범위가 맞지 않다, `timeout`: 앱이 제때 답하지 않았다, `not_found`: 이미 지난 부탁 ID)는 Lucia의 도구 결과로만 가고, 앱에는 오지 않는다.

---

## 16. 기기 역할(`device`)

센서·가젯처럼 사람이 아닌 기기용이다. 관리 페이지 화면에서는 아직 발급하지 않는다.

- 연결과 `hello`는 앱과 같다. `welcome`은 짧다.

```json
{"type": "welcome", "v": 2, "client": "cl-…", "name": "현관 센서", "role": "device",
 "capabilities": {"events": true, "max_event_chars": 1000}}
```

- 받는 프레임은 `event`(5.6과 같은 규칙, 연결마다 한 시간 60개)와 `ping`뿐이다. 그 밖의 프레임은 `forbidden`이다.
- 번호 붙는 프레임, 카드, 메서드, HTTP API는 없다.
- 이벤트 입구가 없는 데몬은 기기 연결을 `unsupported_role`로 거절한다.

---

## 17. 예시 흐름

### 17.1 처음 연결과 대화

```
→ {"type":"hello","token":"lucia_…","resume":0,"features":[]}
← {"type":"welcome","v":2,"client":"cl-3f9a0c12de",…,"seq":0,…}
← {"type":"resumed","from":0,"to":0,"last":0,"truncated":false}
→ {"type":"call","id":"h1","method":"history","params":{"scope":"owner","limit":50}}
← {"type":"result","id":"h1","ok":true,"data":{"items":[…],"more":false}}
→ {"type":"message","text":"내일 일정 알려 줘","id":"m1"}
← {"type":"accepted","id":"m1"}
← {"type":"typing"}
← {"type":"text","seq":1,"body":"내일은 10시에 …"}
→ {"type":"ack","seq":1}
```

### 17.2 승인 카드

```
← {"type":"card","seq":2,"ref":"app:cl-3f9a0c12de:5b0e1f3a9c2d","card":{…,"buttons":[{"action":"approve:apr-1a2b",…},{"action":"reject:apr-1a2b",…}]}}
→ {"type":"ack","seq":2}
→ {"type":"press","ref":"app:cl-3f9a0c12de:5b0e1f3a9c2d","action":"approve:apr-1a2b"}
← {"type":"card_closed","seq":3,"ref":"app:cl-3f9a0c12de:5b0e1f3a9c2d","card":{"headline":"✅ 보냈습니다",…,"buttons":[]}}
→ {"type":"ack","seq":3}
```

### 17.3 앱이 꺼져 있을 때

```
(앱 꺼짐) 데몬: 브리핑 → 상자 seq 4 → FCM {"data":{"type":"wake","kind":"text","seq":"4",…}}
(사용자가 알림을 누름)
→ {"type":"hello","token":"lucia_…","resume":3}
← {"type":"welcome",…,"seq":4}
← {"type":"text","seq":4,"replay":true,"body":"좋은 아침이에요. 오늘은 …"}
← {"type":"resumed","from":3,"to":4,"last":4,"truncated":false}
→ {"type":"ack","seq":4}
```

### 17.4 파일 보내기

```
POST /v1/files?name=%EC%98%81%EC%88%98%EC%A6%9D.jpg   (본문: JPEG)
← 201 {"id":"f_0a1b2c3d4e5f","name":"영수증.jpg","size":482113,"mime":"image/jpeg","kind":"image"}
→ {"type":"message","text":"이거 정리해 줘","files":["f_0a1b2c3d4e5f"],"id":"m2"}
← {"type":"accepted","id":"m2"}
```

### 17.5 사이트 로그인(입력 카드 → 인증 코드 → 사람 확인)(결정 0063)

알림 받을 곳이 앱이고 `browser_handover` 범위가 켜져 있다고 하자. Lucia가 사이트에 로그인이 필요해 멈추면:

```
← {"type":"card","seq":10,"ref":"app:cl-3f9a0c12de:5b0e1f3a9c2d","card":{"title":"로그인 정보 입력",…,
    "buttons":[{"action":"approval:3f2a…:approve","label":"로그인 정보 입력","form":{"title":"로그인 정보 입력","fields":[…]},…},
               {"action":"approval:3f2a…:deny","label":"취소","form":null,…}]}}
→ {"type":"ack","seq":10}
(사용자가 입력창에 아이디·비밀번호를 넣고 확인. 앱은 확인할 때까지 값을 메모리에만 둔다)
→ {"type":"press","ref":"app:cl-3f9a0c12de:5b0e1f3a9c2d","action":"approval:3f2a…:approve",
   "inputs":{"username":"me","password":"…","save":true}}
← {"type":"card_closed","seq":11,"ref":"app:cl-3f9a0c12de:5b0e1f3a9c2d",
   "card":{"headline":"⏳ 로그인하고 있습니다. 결과는 곧 알려 드립니다.",…,"buttons":[]}}
→ {"type":"ack","seq":11}
(로그인에 몇 초에서 수십 초. 이 사이 앱은 아무것도 보내지 않는다. 결과는 아래 가운데 하나다. 전체는 9.1의 표)
```

가. 성공:

```
← {"type":"text","seq":12,"body":"✅ 예시 사이트에 로그인했습니다. 다음부터는 저장된 로그인을 승인만 하면 됩니다. 하던 일을 다시 말씀해 주세요."}
```

나. 인증 코드(OTP)가 필요하다(`prompt`가 있는 카드, 5.6의 `input`):

```
← {"type":"card","seq":12,"ref":"app:cl-3f9a0c12de:7c41d2e8b09f","card":{"title":"인증 코드 입력",…,
    "buttons":[{"label":"코드 입력","prompt":{"title":"인증 코드 입력","label":"인증 코드",…},"form":null,…},{"label":"취소",…}]}}
→ {"type":"press","ref":"app:cl-3f9a0c12de:7c41d2e8b09f","action":"approval:…:approve","input":"123456"}
← {"type":"card_closed","seq":13,"ref":"app:cl-3f9a0c12de:7c41d2e8b09f","card":{…}}
```

다. 사이트가 사람 확인(캡차)을 띄웠다. 데몬은 알림 글만 보내고, 화면은 앱이 연다(6.7):

```
← {"type":"text","seq":12,"body":"🧩 예시 사이트이(가) 사람 확인(…)을 띄워 아직 로그인되지 않았습니다. 10분 안에 앱의 브라우저 화면 넘겨받기(또는 관리 페이지 \"브라우저 넘겨받기\")에서 그 화면을 직접 풀어 주세요(…): https://…/browser/handover"}
→ {"type":"call","id":"b1","method":"browser.handover.start"}
← {"type":"result","id":"b1","ok":true,"data":{"active":true,"left_seconds":899,"max_seconds":900,"tabs":[{"id":"6F1A…","url":"https://…","title":"…"}]}}
GET /v1/browser/handover/frame/6F1A…            → 200 image/jpeg
→ {"type":"call","id":"b2","method":"browser.handover.input","params":{"tab":"6F1A…","kind":"click","x":412,"y":288}}
← {"type":"result","id":"b2","ok":true,"data":{"sent":true}}
GET /v1/browser/handover/frame/6F1A…            → 200 image/jpeg (0.4초쯤 뒤)
(… 사용자가 사람 확인을 푼다 …)
→ {"type":"call","id":"b9","method":"browser.handover.end"}
← {"type":"result","id":"b9","ok":true,"data":{"active":false,"warning":null}}
```

라. 실패해서 다시 넣을 수 있다(시도는 3번까지). 새 카드가 온다:

```
← {"type":"text","seq":12,"body":"⚠️ 예시 사이트 로그인하지 못했습니다: …. 새로 올린 카드에서 다시 넣을 수 있습니다."}
← {"type":"card","seq":13,"ref":"app:cl-3f9a0c12de:9d2b6a41c0e7","card":{…,"fields":[…,{"name":"앞선 시도","value":"⚠️ …","inline":false}],…}}
```

### 17.6 사용자에게 묻는 앱 기능과 파일(결정 0066)

`hello.tools`에 `screen.capture`(`when: "here"`, `timeout: 90`)를 내준 데스크톱 앱이다. 사용자가 이 앱에서 Lucia와 대화 중이고 `app_tools` 범위가 켜져 있다.

```
← {"type":"invoke","id":"d-7a1c0e9b3f52","method":"screen.capture","params":{"target":"screen"},"timeout":90}
(Lucia의 도구는 45초 뒤 부탁 ID를 받고 다른 일을 한다. 데몬은 90초까지 기다린다. 사용자가 앱의 확인 창에서 허락한다)
POST /v1/files?name=screen.png&keep=short   (본문: PNG)
← 201 {"id":"f_b7f4006bdf27","name":"screen.png","size":482113,"mime":"image/png","kind":"image","expires_at":"2026-11-22T08:41:00.123456789+00:00"}
→ {"type":"result","id":"d-7a1c0e9b3f52","ok":true,"data":{"width":2560,"height":1440},"files":["f_b7f4006bdf27"]}
```

사용자가 거절했으면 `{"ok": false, "error": {"code": "denied", "message": "사용자가 거절"}}`, 확인 창을 끝내 보지 않았으면 `"code": "timeout"`으로 답한다. 데몬이 90초 안에 답을 받지 못해도 Lucia는 `timeout`을 받는다.

### 17.7 휴대폰과 데스크톱이 한 대화를 쓸 때(결정 0066·0067)

데스크톱(푸시 없음, `seen`을 말한다)과 휴대폰(푸시 있음)이 붙어 있고, 알림 받을 곳은 "앱 모두"다. 사용자가 자리에 있다.

**브리핑**: 데스크톱이 `background`다.

```
(Lucia: 브리핑 → 둘의 상자에 넣는다. 앞에 있는 앱이 없고 푸시 없는 데스크톱이 연결돼 있어 데스크톱이 울린다)
데스크톱 ← {"type":"text","seq":31,"body":"회의 10분 전이에요"}
휴대폰   ← {"type":"text","seq":77,"body":"회의 10분 전이에요","alert":false}      (끊겨 있으면 상자에만 쌓인다. 푸시 없음)
(사용자가 말풍선을 눌러 본다)
데스크톱 → {"type":"seen","seq":31}
(휴대폰이 끊겨 있다) 푸시 {"data":{"type":"clear","seq":"77","client":"cl-phone…"}}   (운영체제 알림이 없으면 아무 일 없다)
```

**휴대폰에서 묻는다**: 데스크톱 창이 앞에 있다. 휴대폰은 말한 뒤 주머니에 넣어 끊겼다.

```
휴대폰   → {"type":"message","text":"내일 일정 알려 줘","id":"m1"}
데스크톱 ← {"type":"sent","seq":40,"body":"내일 일정 알려 줘","conversation":"app:cl-phone…",
            "from":{"client":"cl-phone…","name":"휴대폰"},"at":"2026-11-22T08:12:03+00:00"}   (사용자 자신의 말풍선으로 그린다. 울리지 않는다)
데스크톱 ← {"type":"typing"}
데스크톱 ← {"type":"text","seq":41,"body":"내일은 10시에 …","conversation":"app:cl-phone…","alert":false}
휴대폰   ← 푸시 {"notification":{…},"data":{"type":"wake","kind":"text","seq":"90",…}}
```

물은 앱(휴대폰)이 푸시를 받는 앱이고 앞에 있지 않아서, 데스크톱 창이 앞에 있어도 휴대폰이 먼저 울린다(⓪). 답은 데스크톱의 상자에도 `alert: false`로 들어가 대화 화면에 놓인다. 휴대폰에서 열어 보면 휴대폰이 `seen{90}`을 보내고, 데몬이 데스크톱에도 `seen{41}`을 알린다(놓아 둔 프레임이라 치울 알림은 없다).

**데스크톱에서 묻는다**: 데스크톱이 앞에 있고 휴대폰은 끊겨 있다.

```
데스크톱 → {"type":"message","text":"회의 자료 정리해 줘","id":"m7"}
휴대폰   ← {"type":"sent","seq":91,"body":"회의 자료 정리해 줘","conversation":"app:cl-desk…",
            "from":{"client":"cl-desk…","name":"데스크톱"},"at":"…"}    (상자에만 쌓인다)
데스크톱 ← {"type":"text","seq":52,"body":"정리했어요"}                    (물은 앱이 앞에 있다: 데스크톱이 울린다)
휴대폰   ← {"type":"text","seq":92,"body":"정리했어요","conversation":"app:cl-desk…","alert":false}   (상자에만 쌓인다. 푸시 없음)
```

사용자가 말풍선을 보면 `seen`이 가고 끝난다(휴대폰이 끊겨 있으면 `clear` 푸시). 보지 않고 자리를 비우면:

```
데스크톱 → {"type":"presence","state":"away"}       (그 앱이 울려 놓고 보지 않은 알림을 바로 넘긴다. 앞에 계속 있으면 2분 뒤에)
휴대폰   ← {"type":"alert","seq":92}                 (연결돼 있으면: 놓아 둔 92번을 지금 알린다)
휴대폰   ← 푸시 {"notification":{…},"data":{"type":"wake","kind":"text","seq":"92",…}}    (앞에 있지 않으면: 끊겨 있거나 `background`·`away`)
(사용자가 휴대폰에서 연다)
휴대폰   → {"type":"seen","seq":92}
데스크톱 ← {"type":"seen","seq":52}                  (데스크톱이 연결돼 있으면. 끊겨 있으면 clear 푸시)
```

복사해 넣는 일이 없으므로 넘길 때도 92번은 처음부터 휴대폰의 상자에 있던 프레임이다.

---

## 18. 판 1에서 바뀐 점

| 판 1(결정 0021) | 판 2(결정 0043) |
|---|---|
| 번호·ack 없음, 끊긴 동안 잃음 | `seq`·`ack`·`hello.resume`·`resumed` |
| 파일을 base64로 프레임에 실음 | 파일은 ID로 알리고 HTTP로 받음 |
| 앱에서 파일 보내기 없음 | `POST /v1/files` + `message.files` |
| HTTP API 없음 | `/v1/me`, `/v1/history`, `/v1/files`, `/v1/push` |
| 메서드·`invoke` 없음 | `call`/`result`, `invoke`, `hello.features` |
| 앱 이벤트 없음 | 앱도 `event`를 보냄 |
| 끊긴 앱의 글은 늘 Discord로 | 푸시를 쓸 수 있으면 상자에 쌓고 FCM으로 깨움 |
| 승인 카드는 늘 Discord로 | "알림 받을 곳"을 앱으로 정할 수 있음 |
| 집 LAN·VPN에서만 | 설정하면 리버스 프록시 뒤 공개 주소로도 |
