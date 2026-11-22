# 서버가 계산해 주는 값(`ui`, `consts`)

예전 화면 코드(`crates/luciad/src/admin/view/*.rs`, 지금은 시험에서만 컴파일된다)는 이름·결·설명·한도를 **데몬의 Rust 코드를 불러** 얻었다.

> 예전 화면 코드(`crates/luciad/src/admin/view/*.rs`)는 화면을 다 옮긴 뒤 지웠다(결정 0061). 아래 "바꾼 호출" 칸의 파일과 줄 번호는 그때의 것이고(커밋 `7eafdc8`에서 볼 수 있다), 값이 어디서 왔는지 알려 주는 기록이다.
브라우저는 Rust를 부를 수 없고, 그 표를 TypeScript로 다시 만들지도 않는다. 그래서 데몬(`crates/luciad/src/admin/derive.rs`, `extra.rs`)이 그 값을 JSON에 실어 보낸다.
이 문서는 **무엇이 JSON 어디에 실리는지**와 **그것이 바꾼 예전 호출**을 화면 파일마다 적는다. 화면을 짜는 쪽은 이 문서만 보고 값을 찾는다.

## 0. 읽는 법

### 값이 오는 곳

| 이름 | 어디 | 뜻 |
|---|---|---|
| `snapshot.<목록>[i].ui` | `data.snapshot` 안 목록의 i번째 항목 | 그 항목 하나에서 나오는 값 |
| `snapshot.<객체>.ui` | `discord`, `google`, `backup`처럼 객체 하나 | 그 객체에서 나오는 값 |
| `snapshot.ui` | `data.snapshot.ui` | 스냅숏 여러 칸을 합쳐야 나오는 값 |
| `name` | `data.name` | 화면 전체의 값: 관리 화면과 앱에 보이는 이름(14장) |
| `consts` | `data.consts` | 데이터와 상관없는 표·한도·이름표 |
| `extra…` | `data.extra` | 화면마다 다른 덧붙임(기억 항목, 스킬, 캐릭터). 그 안의 항목에도 `ui`가 있다 |

- 짝 배열 두 개는 **첫 요소**에 `ui`가 붙는다: `snapshot.watches[i][0].ui`(`[감시, 원천 상태 | null]`), `snapshot.clients[i][0].ui`(`[클라이언트, 연결됨]`).
- `goals[i]`는 `{ goal, log, children }`이고 `ui`는 `goals[i].ui`다(`goals[i].goal.ui`가 아니다).
- 목표의 `children`, 루틴의 `runs`, 목표의 `log`처럼 안쪽 목록의 항목에는 바깥 코드를 부른 값이 없어 `ui`가 없다.
- `snapshot.expressions`는 `null`일 수 있다(이모지를 다루지 않는 데몬). `ui`는 `expressions.emoji[i]`, `expressions.stickers[i]`에 붙는다.

### 이름 규칙

- `*_label`, `label`, `summary`, `text`로 끝나는 글은 **화면에 그대로 적는 글**이다.
- `tone`은 `"ok" | "muted" | "info" | "warn" | "bad"`(`Badge`·`Dot`의 `tone`과 같은 말)다.
- `consts`의 표 행 `key`와 항목 `ui`의 `*_key`·`security_mode`는 **같은 값**이라 `consts` 표에서 찾을 때 쓴다. 이 값은 폼이 보내는 값(`as_str()`)과도 같다.
- `null`은 "없음/모름"이다. 문서에 `string | null`이라 적은 칸은 `null`일 때 그 부분을 그리지 않는다(없으면 원문을 쓰라고 적은 칸은 그렇게 한다).
- `ui` 안의 불리언(`can_*`, `is_*`, `uses_*`, `open`, `pending`, `active`, `usable`, `deleted`, `editable`)은 열거형 비교를 대신한다. **열거형 원본을 비교하지 않고 이 불리언을 쓴다.**
- 숫자·돈·시각 표기(`num`, `usd`, `bytes`, `when`, `exact`)는 `src/lib/format.ts`에 있다. 여기서 다시 주지 않는다.
- 글을 `n`자로 자르는 `chars().take(n)`(`view/work.rs:134,198`, `view/activity.rs:120`)은 유니코드 글자 단위이고 화면이 한다. 같은 결과를 내려면 `[...글].slice(0, n).join('')`을 쓴다(`String.prototype.slice`는 UTF-16 단위라 이모지가 깨진다).
- 문자열의 UTF-8 바이트 수가 필요한 곳(`view/system.rs:78`, `view/lucia.rs:329,377`)은 서버가 숫자로 준다(`snapshot.ui.memory_bytes`, 캐릭터는 6장). 브라우저에서 `.length`로 세지 않는다.
- 그림 주소 뒤의 캐시 방지 값(`view/browser.rs:247`의 `chrono::Utc::now().timestamp_millis()`)은 요청할 때마다 새로 정하는 값이라 화면이 `Date.now()`로 정한다.

### 열거형의 원본 모양(비교하지 않기를 권하지만 알아 둘 것)

저장소의 열거형은 **대문자로 시작하는 변형 이름**으로 직렬화된다(`"Active"`). 예외가 있어 적는다.

| 칸 | 값 |
|---|---|
| `approvals[i].status` | `"Pending" "Approved" "Sent" "Denied" "Expired" "Failed"` (글·결은 `ui.status`, `ui.tone`) |
| `goals[i].goal.status` | `"Active" "Waiting" "Done" "Abandoned"` |
| `routines[i].routine.status` | `"Review" "Active" "Paused" "Done"` |
| `watches[i][0].status` | `"Pending" "Active" "Paused" "Removed"` |
| `followups_*[i].status` | `"Open" "Done" "Cancelled" "Failed"` |
| `tasks_*[i].status` | **snake_case**: `"created" "running" "waiting_approval" "waiting_input" "reported" "done" "failed" "cancelled" "expired"` |
| `memories[i].section`, 기억 항목 | `"Me" "Topic" "Area"` (`ui.section_key`는 `"me" "topic" "area"`) |
| 기억 개정 `action` | `"Write" "Delete"` |
| `snapshot.security_mode` | `"Strict" "Standard" "Open"` (`snapshot.ui.security_mode`는 `"strict" "standard" "open"`) |
| `routines[i].routine.trigger` | `{ "Cron": "식" }`, `{ "Every": { "minutes": n, "window": "HH:MM-HH:MM" \| null } }`, `{ "Change": "원천" }` (글은 `ui.trigger_label`) |
| `discord.state` | `"NotConfigured"`, `{ "Connecting": { "bot" } }`, `{ "Connected": { "bot" } }`, `{ "Stopped": { "reason" } }` (`discord.ui.state`가 풀어 준다) |

---

## 1. 공통 틀 (`view/ui.rs`와 모든 화면이 쓰는 것)

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `snapshot.activity[i].ui.icon` | `string`. 활동 종류의 아이콘 이름(`Icon name`이 받는 이름: `search mail calendar send x-circle shield memory chat clock link upload smile spark activity`). 모르는 종류는 `activity` | `view/ui.rs:107` `activity_icon(&a.kind)` (쓰는 곳 `view/home.rs:182`, `view/activity.rs:51`) |
| `snapshot.activity[i].ui.detail` | `string`. 요약이 종류 이름(`ui.label`)으로 시작하면 그 앞부분과 `:`·공백을 뺀 글. 제목 아래 작은 글로 쓴다 | `view/ui.rs:633` `activity_detail(&a.kind, &a.summary)` (`view/home.rs:183`, `view/activity.rs:52`) |
| `consts.pages[]` | `{ key, path, label, nav_label, tab_label }[]`, `Page::ALL` 순서. `key`는 응답의 `page`와 같다(`"overview" "work" … "safety" "notify" "tasks" "system" "start"`). `label`은 화면 제목, `nav_label`은 메뉴 이름(설정 셋은 모두 `"설정"`), `tab_label`은 보조 탭 이름 | `view/ui.rs:454` `p.tab_label()`, `view/ui.rs:529` `p.path()`, `view/ui.rs:531` `p.nav_label()`, 화면 제목 `p.label()`; `view/home.rs:42,50,59,72` 등 `Page::X.path()` |
| `consts.settings_tabs` | `string[]`. 설정의 보조 탭 `key`들, 순서대로 `["safety","notify","tasks"]` | `view/settings.rs:13` `Page::SETTINGS` (`view/ui.rs:450` `subnav`) |

이미 있던 것: `data.notices`, `data.overall`, `data.pending`(`view/ui.rs:564-570`의 `notices(s)`, `overall(&found)`, 승인 대기 수), `data.title`·`data.path`(`page.label()`, `page.path()`).
`Tone` 값은 `Tone::class()`와 같은 글이라(`"ok"` …) 그대로 `tone`에 쓴다.

**주지 않은 것**

| 템플릿 | 호출 | 까닭 |
|---|---|---|
| `view/ui.rs:98` | `tone_icon(tone)` | 결 → 아이콘 이름 표시뿐이다(`ok`→`check-circle`, `bad`→`x-circle`, `warn`→`alert`, `info`·`muted`→`info`). 화면이 한다 |
| `view/ui.rs:488` | `nav_icon(page)` | 메뉴 아이콘은 `src/lib/nav.ts`가 갖고 있다 |
| `view/ui.rs:506` | `NAV`(메뉴 묶음) | 같음 |
| `view/ui.rs:529` | `p.is_nav_current(page)`(`admin/snapshot.rs`) | 설정 화면 셋이 한 칸을 나눠 쓰는 규칙이다. `nav.ts`의 `also`가 한다 |

---

## 2. 홈 (`view/home.rs`)

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `snapshot.discord.ui.status`, `.tone`, `.bot` | 연결 상태 글·결, 봇 이름(`string \| null`) (이전부터) | `view/home.rs:17` `discord_status(&s.discord.state)`, `view/home.rs:46` `discord_bot(&s.discord.state)` |
| `snapshot.google.ui.status`, `.tone` | Google 연결 상태 글·결. 오류 > 읽기만 > 연결됨 > 동의 전 > 연결 안 됨 순으로 정한 값 | `view/home.rs:18` `google_status(&s.google)` (`view/connections.rs:13`) |
| `snapshot.schedules[i].ui.label` | 루틴 이름(이전부터) | `view/home.rs:75` `routine_label(&r.id)` |
| `snapshot.goals[i].ui.open` | `boolean`. 진행 중이거나 사람 차례인 목표 | `view/home.rs:139,151` `v.goal.status.open()` |
| `snapshot.activity[i].ui.label`, `.tone`, `.icon`, `.detail` | 제목, 결, 아이콘, 작은 글(`label`·`tone`은 이전부터) | `view/home.rs:181-183` `activity_label`, `activity_tone(..).class()`, `activity_icon`, `activity_detail` |
| `snapshot.approvals[i].ui.pending` | `boolean`. 승인 대기인가 | `view/home.rs:27` `a.status == ApprovalStatus::Pending` |
| `consts.security.modes[]` | `{ key, label, icon, summary, items: string[], keeps: string[] }[]`, 순서 엄격·기본·해제. `key`는 `"strict" "standard" "open"`(폼 `mode`의 값). `icon`은 아이콘 이름(`lock`·`shield`·`unlock`. 이모지가 아니다: 결정 0062). `items`는 그 모드가 달라지는 것(엄격 `STRICT_ITEMS`, 해제 `OPEN_ITEMS`, 기본은 `[]`), `keeps`는 해제에서도 그대로 두는 것(해제 `OPEN_KEEPS`, 나머지 `[]`) | `view/home.rs:234` `Mode::ALL`, `:236` `m.icon()`·`m.label()`, `:237` `m.summary()`, `:241` `m.as_str()`, `:252` `STRICT_ITEMS`, `:254` `OPEN_ITEMS`, `:256` `OPEN_KEEPS` |
| `snapshot.ui.security_mode` | `"strict" \| "standard" \| "open"`. 지금 모드. `consts.security.modes`의 `key`와 맞춘다 | `view/home.rs:228` `now.label()`, `:235,238` `m == now` |

권한 모드 패널의 `full`(설정 화면)에서 세 목록의 제목은 템플릿에 적힌 글이다: "엄격 [제안]" = `modes[strict].items`, "해제(사람이 대화 중인 실행만)" = `modes[open].items`, "해제에서도 그대로 두는 것" = `modes[open].keeps`.

**주지 않은 것**: 없음. `upcoming_panel`의 행 만들기(`view/home.rs:114-170`)와 건수 세기는 스냅숏 값만 읽는다(목표는 위 `ui.open`을 쓴다).

---

## 3. 할 일 (`view/work.rs`)

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `snapshot.goals[i].ui.open` | `boolean` | `view/work.rs:9,29,47` `g.status.open()` |
| `snapshot.goals[i].ui.status_label` | `string`. "진행 중", "사람 차례", "완료", "포기" | `view/work.rs:48` `g.status.label()` |
| `snapshot.nudges[i].ui.stage_label` | `string`. `stage`가 0이면 "아직 알리지 않음", 1 "소식 보냄", 2 "알림 보냄", 그 밖 "확인 카드 보냄" | `view/work.rs:108` `match n.stage { 0 => …, 1 => …, 2 => …, _ => … }` |
| `snapshot.tasks_open[i].ui.status_label` | `string`. 작업 상태 이름("대기", "하는 중", …) | `view/work.rs:132` `t.status.label()` |
| `snapshot.tasks_closed[i].ui.status_label` | 같음 | `view/work.rs:147` `t.status.label()` |
| `snapshot.tasks_closed[i].ui.tone` | `Tone`. 완료면 `ok`, 아니면 `warn`. (`tasks_open`에도 같은 칸이 있으나 쓰지 않는다) | `view/work.rs:143` `t.status == lucia_core::TaskStatus::Done` |
| `snapshot.followups_open[i].ui.uses_browser` | `boolean`. 이 후속 조치가 그 시각에 브라우저(누르기·입력)를 쓸 수 있나. `false`이면 "브라우저 허용" 단추를 보인다 | `view/work.rs:172,176` `crate::autonomy::followup_uses_browser(&f.origin)` |
| `snapshot.followups_open[i].ui.origin_label` | `string`. "브라우저 허용함"(사용자가 허용), "대화에서 등록(브라우저 사용)", "브리핑에서 등록(위임 사이트 읽기만)" | `view/work.rs:171-173` `f.origin == crate::autonomy::ORIGIN_USER` + `followup_uses_browser` |
| `snapshot.followups_closed[i].ui.status_label` | `string`. "예정", "함", "취소", "실패" | `view/work.rs:196` `f.status.label()` |
| `snapshot.followups_closed[i].ui.tone` | `Tone`. 실패 `bad`, 함 `ok`, 그 밖 `muted` | `view/work.rs:189-190` `lucia_store::FollowupStatus::Failed/Done => Tone::…` |

`followups_open[i].ui`와 `followups_closed[i].ui`는 같은 칸들(`status_label`, `tone`, `uses_browser`, `origin_label`)을 모두 가진다.

**주지 않은 것**: 없음. (`r.chars().take(80)`, `g.interests.replace('\n', ", ")`는 글 다듬기라 화면이 한다.)

---

## 4. 자동화 (`view/automation.rs`)

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `snapshot.routines[i].ui.status_label` | `string`. "승인 대기", "켜짐", "멈춤", "끝남" | `view/automation.rs:32` `r.status.label()` |
| `snapshot.routines[i].ui.tone` | `Tone`. 켜짐 `ok`, 승인 대기 `warn`, 그 밖 `muted` | `view/automation.rs:32` `match r.status { R::Active => Tone::Ok, R::Review => Tone::Warn, _ => Tone::Muted }` |
| `snapshot.routines[i].ui.trigger_label` | `string`. "cron …", "n시간마다(창)", "새 변화마다(…, 사전 체크 때)" | `view/automation.rs:35` `crate::routines::trigger_label(&r.trigger)` |
| `snapshot.routines[i].ui.can_run`, `.can_pause` | `boolean`. 켜짐일 때만 참. "지금 실행"(`/routines/{id}/run`), "멈추기"(`/routines/{id}/pause`) 단추 | `view/automation.rs:43` `R::Active => { run, pause }` |
| `snapshot.routines[i].ui.resume_label` | `string \| null`. 멈춤이면 "다시 켜기", 승인 대기면 "켜기(승인)", 그 밖(켜짐·끝남)은 `null`이고 단추가 없다. 단추는 둘 다 `/routines/{id}/resume` | `view/automation.rs:44-46` `R::Paused => "다시 켜기"`, `R::Review => "켜기(승인)"`, `R::Done => {}` |
| `snapshot.watches[i][0].ui.status_label` | `string`. 멈춘 까닭을 포함한 글("감시 중", "멈춤"(다른 까닭), "멈춤(계속 바뀜)", "멈춤(로그인 필요)", "승인 대기", "지움") | `view/automation.rs:111` `w.status_label()` |
| `snapshot.watches[i][0].ui.tone` | `Tone`. 감시 중 `ok`, 승인 대기 `info`, 그 밖 `warn` | `view/automation.rs:111` `match w.status { W::Active => Tone::Ok, W::Pending => Tone::Info, _ => Tone::Warn }` |
| `snapshot.watches[i][0].ui.is_items` | `boolean`. 게시판(새 글) 감시인가(아니면 구간 감시) | `view/automation.rs:116,178` `w.is_items()` |
| `snapshot.watches[i][0].ui.via_label` | `string`. "로그인 없이 HTTP", "로그인 세션으로 HTTP", "브라우저로 읽기" | `view/automation.rs:118` `crate::watch::Via::parse(&w.fetcher).label()` |
| `snapshot.watches[i][0].ui.notify_label` | `string`. "바로 알림", "작은 모델이 골라 알림", 그 밖 "묶음 소식만" | `view/automation.rs:88` 안의 `notify_label(&w.notify)` (`:117`) |
| `snapshot.watches[i][0].ui.interval_min` | `number`. 이 감시의 간격 하한(분). 게시판 3, 구간 10 | `view/automation.rs:175-176` `crate::watch::interval_min(w.is_items())` |
| `snapshot.watches[i][0].ui.uses_browser` | `boolean`. 브라우저로 읽는 감시인가 | `view/automation.rs:96` `w.fetcher == lucia_store::watch_fetcher::BROWSER` |
| `snapshot.watches[i][0].ui.can_pause`, `.can_resume` | `boolean`. 감시 중이면 "멈추기", 멈춤이면 "다시 켜기" 단추(`/watches/{id}/pause`, `/watches/{id}/resume`) | `view/automation.rs:134-135` `W::Active =>`, `W::Paused =>` |
| `snapshot.watches[i][0].ui.editable` | `boolean`. 승인 대기가 아니면 "고치기"를 보인다 | `view/automation.rs:142` `w.status != W::Pending` |
| `snapshot.ui.browser_watches` | `number`. 켜져 있고 브라우저로 읽는 감시 수("브라우저 상시 켜짐" 안내, 0이면 안 보임) | `view/automation.rs:93-97` `w.status == W::Active && w.fetcher == watch_fetcher::BROWSER` |
| `consts.watch.now_daily_range` | `[number, number]`. 하루 알림 수 입력의 `min`, `max` | `view/automation.rs:153` `crate::watch::NOW_DAILY_RANGE.start()/.end()` |
| `consts.watch.host_gap_minutes` | `number`. 같은 사이트를 가져오는 간격 | `view/automation.rs:161` `crate::watch::HOST_GAP_MINUTES` |
| `consts.watch.interval_max` | `number`. 간격 상한(분) | `view/automation.rs:175-176` `crate::watch::INTERVAL_MAX` |
| `consts.watch.interval_min` | `{ items: number, region: number }`. 방식별 간격 하한(게시판 3, 구간 10). 항목의 `ui.interval_min`이 이미 그 항목의 값을 골라 준다 | `view/automation.rs:175-176` `crate::watch::interval_min(…)` |
| `consts.watch.notify[]` | `{ key, label }[]`. 알림 방식 고르기의 선택지(`key`가 폼 값, `w.notify`와 비교) | `view/automation.rs:187-188` `lucia_store::watch_notify::ALL` + `notify_label(n)` |
| `consts.watch.fetchers[]` | `{ key, label }[]`. 읽는 방식 고르기의 선택지(`key`가 폼 값, `w.fetcher`와 비교) | `view/automation.rs:194-195` `lucia_store::watch_fetcher::ALL` + `Via::parse(f).label()` |
| `consts.watch.browser_fetcher` | `string`. 브라우저로 읽는 방식의 `key` | `view/automation.rs:96` `lucia_store::watch_fetcher::BROWSER` |

**주지 않은 것**: 없음. `routines[i].runs[j]`에는 바깥 호출이 없다.

---

## 5. 활동 (`view/activity.rs`)

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `snapshot.activity[i].ui.groups` | `string[]`. 이 활동이 속한 거르개 `key`들(`"all"`은 늘 들어 있다). 거르개를 고르면 `groups`에 그 `key`가 든 것만 보인다(이전부터) | `view/activity.rs:20` `filter.matches(&a.kind)` |
| `consts.activity_filters[]` | `{ key, label }[]`. 거르개 칩, 순서대로(이전부터) | `view/activity.rs:42-43` `ActivityFilter::ALL`, `f.key()`, `f.label()` |
| `snapshot.activity[i].ui.label`, `.tone`, `.icon`, `.detail` | 제목, 결, 아이콘, 작은 글(`label`·`tone`은 이전부터) | `view/activity.rs:50-52` `activity_label`, `activity_tone(..).class()`, `activity_icon`, `activity_detail` |
| `snapshot.approvals[i].ui.status`, `.tone` | 승인 상태 글·결(이전부터) | `view/activity.rs:76` `approval_status(a.status)` |
| `snapshot.approvals[i].ui.pending` | `boolean`. 승인 대기인가 | `view/activity.rs:15` `a.status == ApprovalStatus::Pending` |
| `snapshot.signals.recent[i].ui.verdict_label` | `string`. "지금 알림", "소식", "버림", "브리핑에서 다룸", 아직 안 나눴으면 "분류 대기" | `view/activity.rs:122-127` `match x.verdict.as_deref() { Some("now") => …, … _ => "분류 대기" }` |

고른 거르개는 `extra.filter`(`"all" "mail" …`, 이전부터)다.

**주지 않은 것**: 없음.

---

## 6. Lucia (`view/lucia.rs`: 기억, 스킬, 캐릭터)

### 기억 목록(`/memory`)

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `snapshot.memories[i].ui.section_key` | `"me" \| "topic" \| "area"`. `consts.memory.sections[].key`와 맞춘다 | `view/lucia.rs:60` `lucia_store::MemorySection::ALL`(항목이 어느 갈래인지 `e.section == section`, `:61`) |
| `snapshot.memories[i].ui.section_label` | `string`. "나", "주제", "영역" | `view/lucia.rs:63` `section.label()` |
| `consts.memory.sections[]` | `{ key, label }[]`. 갈래 순서(나, 주제, 영역) | `view/lucia.rs:60,63` `MemorySection::ALL`, `section.label()` |

### 기억 항목(`/memory/{slug}`, `extra.view == "entry"`)

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `extra.entry.ui.section_key`, `.section_label` | 위와 같음 | `view/lucia.rs:172` `e.section.label()` |
| `extra.history[i].ui.author_label` | `string`. "Lucia"(`agent`), "사용자", "데몬" | `view/lucia.rs:165-168` `author(r.author)` |
| `extra.history[i].ui.origin` | `string`. 바꾼 실행의 출처. `run_id` 없음 "관리 페이지·데몬", `chat-…` "대화 중", `remember-…` "관리 페이지 “기억해 줘”", `briefing-…` "아침 브리핑(사람 없음)", `followup-…` "후속 조치(사람 없음)", `routine-…` "반복 루틴(사람 없음)", `task-…` "맡긴 작업(사람 없음)", `wrapup-…` "하루 정리(사람 없음)", `retro-…` "회고(사람 없음)", 그 밖 "기타 실행" | `view/lucia.rs:141,194` `run_origin(r.run_id.as_deref())` |
| `extra.history[i].ui.deleted` | `boolean`. 지움 개정이면 참. 참이면 " · 지움"을 붙이고 "요약:" 줄을 숨기고 되돌리기 단추도 없다. 거짓이면 요약을 보이고, **맨 위(i=0)가 아닌 개정**만 되돌리기 단추를 보인다(`i > 0`은 화면이 센다) | `view/lucia.rs:196,198,201` `r.action == lucia_store::MemoryAction::Delete / Write` |

### 스킬 목록(`/skills`, `extra.view == "list"`)과 스킬 하나(`extra.view == "detail"`)

스킬 항목(`extra.skills[i]`, 상세는 `extra.skill`)에 모두 같은 `ui`가 붙는다.

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `extra.skills[i].ui.status_label` | `string`. "승인 대기", "시험 중", "활성", "오래됨", "보관" | `view/lucia.rs:237,280` `s.status.label()` |
| `extra.skills[i].ui.tone` | `Tone`. 활성 `ok`, 시험 중·승인 대기 `warn`, 그 밖 `muted` | `view/lucia.rs:219-223,237` `tone(s.status)` |
| `extra.skills[i].ui.usable` | `boolean`. 쓸 수 있는 스킬(시험 중·활성). "n개 사용 중"은 이 값을 센다 | `view/lucia.rs:228` `s.status.usable()` |
| `extra.skills[i].ui.footprint` | `string`. 쓰는 것 한 줄("읽기만(검색·메일·일정·기억)", "도구: …", "사이트: …") | `view/lucia.rs:240,282` `crate::skills::footprint_line(&s.footprint)` |
| `extra.skills[i].ui.source_label` | `string`. `source`가 `"chat"`이면 "사용자 요청", 아니면 "회고" | `view/lucia.rs:287` `if s.source == "chat" { … } else { … }` |
| `extra.skills[i].ui.can_approve` | `boolean`. 승인 대기이면 "승인"(`/skills/{slug}/approve`) | `view/lucia.rs:260` `s.status == S::Pending` |
| `extra.skills[i].ui.can_restore` | `boolean`. 오래됨·보관이면 "되살리기"(`…/restore`), 아니면 "끄기"(`…/archive`). 고정 단추("고정"/"고정 풀기")는 `skill.pinned`로 가른다(`…/pin`, `…/unpin`) | `view/lucia.rs:262` `matches!(s.status, S::Stale \| S::Archived)` |
| `extra.revisions[i].ui.author_label` | `string`. `author`가 `"user"`면 "사용자", 아니면 "Lucia" | `view/lucia.rs:293` `if r.author == "user" { … } else { … }` |

### 캐릭터(`/character`, `extra.view == "edit"`)

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `extra.fingerprint` | `string \| null`. 파일 내용의 지문. 저장 폼의 숨은 입력 `base`의 값(그새 파일이 바뀌었는지 데몬이 본다). 파일을 못 읽었으면 `null` | `view/lucia.rs:345` `fingerprint(content)` |
| `extra.max_bytes` | `number`. 캐릭터 파일 크기 상한(바이트) | `view/lucia.rs:329` `crate::character::MAX_BYTES` |
| `extra.content_bytes` | `number \| null`. 지금 내용의 UTF-8 바이트 수(`"{content_bytes} / {max_bytes}바이트"`). 글자 수와 다르므로 브라우저에서 `.length`로 세지 않는다 | `view/lucia.rs:329` `content.len()` |
| `extra.history[i].ui.bytes` | `number`. 그 개정 내용의 UTF-8 바이트 수 | `view/lucia.rs:377` `r.content.len()` |

이전부터 있던 것: `extra.pace`(고른 간격 `key`), `extra.paces[]`(`{ key, label }`, 간격 고르기의 선택지), `extra.model.picked`(고른 모델 `key | null`), `extra.model.label`(지금 값 글, 힌트), `extra.model.config_default`, `extra.model.choices[]`(`{ key, label, about }`), `extra.model.change_note`.
바꾼 호출: `view/lucia.rs:431-432` `Pace::ALL`, `p.key()`, `p.label()`; `view/lucia.rs:388,391,398` `models.picked()`, `models.label()`, `models.config_default()`; `view/lucia.rs:396,399,408` `CHANGE_NOTE`, `CHOICES`.

**주지 않은 것**

| 템플릿 | 호출 | 까닭 |
|---|---|---|
| `view/lucia.rs:76-130` | `memory_text(&e.body)` | 바깥 코드를 부르지 않는 글 → 마크업 변환이다(아래 문법). 표시 로직이라 화면이 한다 |
| `view/lucia.rs:293` | `r.at.with_timezone(&tz).format("%Y-%m-%d %H:%M")` | 시각 표기다. `exact(at, snapshot.tz).slice(0, 16)`과 같은 글이다(시간대는 `snapshot.tz`) |

`memory_text`의 문법(화면이 똑같이 옮긴다): 글을 줄로 나눠 앞뒤 공백을 자른다. 빈 줄은 건너뛴다. `- `로 시작하면 목록 항목(이어지는 목록 항목은 한 `ul`로 묶는다), `#`으로 시작하면 소제목(`#`들을 떼고 앞뒤 공백을 자른다, `h3`), 나머지는 문단(`p`). 줄 안에서 `[[이름]]`은 `이름`이 **비어 있지 않고 영문 소문자·숫자·`-`로만** 이루어졌을 때 `/memory/이름` 링크(글은 `이름`)가 되고, 아니면 글 그대로 둔다(`[[`는 남는다). `]]`가 없으면 그 뒤는 글 그대로다.

---

## 7. 이모지·스티커 (`view/expressions.rs`)

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `consts.expressions.kinds[]` | `{ key, label, max_count, max_bytes }[]`, 이모지(`"emoji"`)·스티커(`"sticker"`) 순. `label`은 "이모지", "스티커"(구역 제목, 그림의 `alt`의 앞말), `max_count`는 개수 상한("n개 · m개까지"), `max_bytes`는 파일 크기 상한(바이트; 안내문 "256 KiB", "512 KiB") | `view/expressions.rs:96,114` `kind.label()`; `:93-94` `MAX_EMOJI`, `MAX_STICKERS`; `:53-54` 크기 상한 |
| `consts.expressions.max_categories` | `number`. 분류 개수 상한 | `view/expressions.rs:175` `MAX_CATEGORIES` |
| `consts.expressions.keep_days` | `number`. 목록에서 지운 그림을 남기는 날수("30일 동안 남습니다") | `view/expressions.rs:148` 안내문의 "30일"(`crate::expressions::KEEP_DAYS`) |
| `snapshot.expressions.emoji[i].ui.size_text`, `snapshot.expressions.stickers[i].ui.size_text` | `string`. 파일 크기("512 B", "12 KB", "1.5 MB") | `view/expressions.rs:143` `crate::files::human_size(e.file.size)` |
| `snapshot.expressions.emoji[i].ui.origin_label`, `…stickers[i].ui.origin_label` | `string`. `origin`이 `"agent"`면 "Lucia가 등록", 아니면 "관리 페이지에서 등록" | `view/expressions.rs:144` `e.origin == "agent"` |

분류 안의 항목 수, 탭 아이콘 이름 찾기, 분류 이름 찾기(`view/expressions.rs:121-125,182-183`)는 목록 안에서 찾는 일이라 화면이 한다. 아이콘 고르기에서 항목의 종류(`e.kind == "emoji"`, `:208`)는 `kinds[].key`와 `label`로 적는다(이모지는 `:이름:`, 스티커는 "스티커 이름").

**주지 않은 것**

| 템플릿 | 호출 | 까닭 |
|---|---|---|
| `view/expressions.rs:53-54` | 한 변의 범위("32~256px", "160~640px")와 권장 크기 | 템플릿에 글로만 적혀 있고 바깥 코드를 부르지 않는다(`Kind::side()`는 모듈 밖에 열려 있지 않다). 안내문 그대로 옮긴다 |

---

## 8. 연결 (`view/connections.rs`)

### Discord

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `snapshot.discord.ui.status`, `.tone`, `.bot` | 연결 상태 글·결, 봇 이름(`string \| null`)(이전부터) | `view/connections.rs:51` `discord_status(&d.state)`, `:108` `discord_bot(&d.state)` |
| `snapshot.discord.ui.state` | `"not_configured" \| "connecting" \| "connected" \| "stopped"`. `"stopped"`이면 패널도 `tone="bad"`, 토큰 폼을 처음 단계로 보인다 | `view/connections.rs:97,113` `matches!(d.state, DiscordState::Stopped { .. })` |
| `snapshot.discord.ui.stopped_reason` | `string \| null`. 멈췄을 때만 그 까닭(멈춤 안내와 토큰 폼) | `view/connections.rs:104` `if let DiscordState::Stopped { reason } = &d.state` |

### Google

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `snapshot.google.ui.status`, `.tone` | 패널 배지의 글·결. "확인 오류"(`bad`), "읽기만"(`warn`), "연결됨"(`ok`), "동의 전"(`warn`), "연결 안 됨"(`muted`). 결이 `bad`이면 패널도 `tone="bad"` | `view/connections.rs:13-29,454` `google_status(g)` |
| `snapshot.google.ui.scopes[]` | `{ label, granted }[]`. 받은 권한 배지들(메일 읽기, 캘린더 읽기, 메일 발송, 캘린더 쓰기 순). 범위를 모르면 `[]`(그러면 "알 수 없음" 배지)(이전부터) | `view/connections.rs:491` `g.scope_list()` |
| `snapshot.google.ui.has_gmail_send` | `boolean`. 메일 발송 권한이 있나(없으면 안내) | `view/connections.rs:505` `g.has_scope(SCOPE_GMAIL_SEND)` |
| `snapshot.google.ui.has_calendar_write` | `boolean`. 캘린더 쓰기 권한이 있나(메일 발송 권한이 있을 때만 이 안내를 본다) | `view/connections.rs:507` `g.has_scope(SCOPE_CALENDAR_WRITE)` |

### 앱·기기 연결(접속 API 클라이언트)

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `snapshot.clients[i][0].ui.role_label` | `string`. "앱", "기기", 모르면 "?" | `view/connections.rs:222` `Role::parse(&c.role).map(\|r\| r.label()).unwrap_or("?")` |
| `snapshot.clients[i][0].ui.is_app` | `boolean`. 역할이 앱인가(푸시·범위·알림 받는 곳은 앱에만) | `view/connections.rs:216,281` `c.role == Role::App.as_str()` |
| `snapshot.clients[i][0].ui.has_security_mode` | `boolean`. "권한 모드 바꾸기" 범위가 있나(범위 폼의 체크 상자 `scope_security_mode`) | `view/connections.rs:217` `c.scopes.iter().any(\|x\| x == scopes::SECURITY_MODE)` |
| `snapshot.clients[i][0].ui.has_app_tools` | `boolean`. "앱 기능 부르기" 범위가 있나(`scope_app_tools`) | `view/connections.rs:218` `… == scopes::APP_TOOLS` |
| `snapshot.clients[i][0].ui.has_browser_handover` | `boolean`. "브라우저 화면 넘겨받기" 범위가 있나(범위 폼과 발급 폼의 체크 상자 `scope_browser_handover`, 결정 0063) | 새 값(예전 화면 코드에 없다). `admin/derive.rs` `decorate_connections`: `… == scopes::BROWSER_HANDOVER` |
| `snapshot.clients[i][0].ui.has_owner_files` | `boolean`. "Discord DM의 파일 받기" 범위가 있나(범위 폼과 발급 폼의 체크 상자 `scope_owner_files`, 결정 0066·0067: 다른 앱 대화의 파일은 범위 없이 받는다) | 새 값(예전 화면 코드에 없다). `admin/derive.rs` `decorate_connections`: `… == scopes::OWNER_FILES` |
| `snapshot.clients[i][0].ui.is_notify_target` | `boolean`. "알림 받는 곳" 배지를 붙일 앱인가: 취소하지 않은 앱이고, 알림을 앱으로 받을 때(결정 0066·0067: 앱 하나만 고르는 것은 없어 취소하지 않은 앱 모두에 붙는다). 화면은 `snapshot.app.notify_client`를 `c.id`와 견주지 않고 이 값을 쓴다 | 새 값. `admin/derive.rs` `decorate_connections`(예전 화면의 `notify_client == c.id`를 대신한다) |
| `snapshot.clients[i][0].ui.scope_labels` | `string[]`. `scopes`와 같은 순서의 이름("권한 모드 바꾸기(/권한)", "Lucia가 앱 기능 부르기", "브라우저 화면 넘겨받기", "Discord DM의 파일 받기"; 모르는 범위는 이름 그대로). `scopes`가 비면 `[]`이고 그때는 "기본(대화·카드·기록·파일·푸시)"를 적는다 | `view/connections.rs:187-192,236` `scope_label(sc)` |
| `snapshot.clients[i][0].ui.has_push_token` | `boolean`. 푸시 등록 토큰이 있나(토큰 자체는 JSON에 없다). "시험 푸시" 단추는 `snapshot.app.fcm_ready`이고 이 값이 참일 때만 | `view/connections.rs:249` `c.push_token.is_some()` |
| `consts.gateway.roles[]` | `{ key, label }[]`. `"app"` "앱", `"device"` "기기" | `view/connections.rs:222` `Role::label` |
| `consts.gateway.app_role` | `string`. 앱 역할의 `key`(`"app"`) | `view/connections.rs:216,281` `Role::App.as_str()` |
| `consts.gateway.scopes[]` | `{ key, label }[]`. 범위 이름표(`"security_mode"`, `"app_tools"`, `"browser_handover"`, `"owner_files"`) | `view/connections.rs:188` `crate::gateway::scopes::ALL` |
| `consts.gateway.scope_security_mode`, `.scope_app_tools`, `.scope_browser_handover`, `.scope_owner_files` | `string`. 네 범위의 `key` | `view/connections.rs:217-218` `scopes::SECURITY_MODE`, `scopes::APP_TOOLS`. `scope_browser_handover`는 새 값(`admin/derive.rs` `gateway_table`, 결정 0063), `scope_owner_files`도 새 값(결정 0066) |
| `consts.gateway.all_apps` | `string`. 알림 받을 곳 고르기(`/app/notify`의 `client`)의 "앱 모두" 값(`"*"`). 선택지는 Discord DM(빈 값)과 이것 둘뿐이고(앱 하나를 고르는 선택지는 결정 0067에서 없앴다), 이것은 앱이 하나라도 있을 때만 보인다. `snapshot.app.notify_client`는 앱으로 받으면 늘 이 값이고 Discord DM이면 `null`이다(저장된 값이 예전의 앱 ID여도 데몬이 이 값으로 읽는다) | 새 값(`admin/derive.rs` `gateway_table`, `crate::gateway::ALL_APPS`, 결정 0066) |

### 이미지 생성

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `snapshot.images.recent[i].ui.label`, `.tone` | `status`가 `"done"`이면 "만듦"(`ok`), `"running"`이면 "만드는 중"(`info`), 그 밖 "실패"(`bad`) | `view/connections.rs:366-370` `match g.status.as_str() { "done" => …, "running" => …, _ => … }` |
| `consts.images.default_daily_usd` | `number`. 하루 예산을 비웠을 때의 기본값(달러). 안내문 "비우면 기본값(…)"에 `usd()`로 적는다 | `view/connections.rs:356` `crate::imagegen::DEFAULT_DAILY_USD` |

**주지 않은 것**: 없음. (`view/connections.rs:207`의 `u.trim_end_matches('/').replacen("https://", "wss://", 1)`는 글 바꾸기뿐이라 화면이 한다.)

---

## 9. 브라우저 (`view/browser.rs`)

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `snapshot.site_grants[i].ui.active` | `boolean`. 지금 쓸 수 있는 위임인가(거두지 않았고, 기한이 남았고, 기한 없는 것은 오래 놀지 않았다). 위임 목록과 "n곳"은 이 값이 참인 것만 보인다 | `view/browser.rs:25,96` `g.active(s.now)` |
| `snapshot.site_grants[i].ui.scope_text` | `string`. 위임 범위를 말로 | `view/browser.rs:108` `crate::delegation::scope_text(g.login, g.actions)` |
| `snapshot.site_grants[i].ui.origin_label` | `string`. `origin`이 `chat` "대화에서 위임", `egress` "카드에서 계속 허용", `admin` "관리 페이지에서 위임", `once` "이번만", `watch` "로그인 감시 승인", 그 밖 "위임" | `view/browser.rs:111-117` `match g.origin.as_str() { … }` |
| `consts.browser.handover_max_secs` | `number`. 화면 넘겨받기의 최대 시간(초, 900). 안내문의 "최대 15분" | `view/browser.rs:268` 안내문의 "15분"(`crate::browser::tools::HANDOVER_MAX`) |
| `consts.browser.handover_keys[]` | `{ key, label }[]`. 넘겨받기의 키 단추(Enter, Tab, ⌫, Esc, ↑, ↓). `key`가 폼 `key`의 값 | `view/browser.rs:258` `[("enter", "Enter"), …]` |

`extra.view == "handover"`의 값(`available`, `left_secs`, `tabs[]`, `tab`, `auto`, `refresh`)은 바깥 호출 없이 처리기가 주는 것이다. 남은 시간 "n분 m초"는 `left_secs`로 화면이 나눈다.

**주지 않은 것**: 없음. (`view/browser.rs:261-262`의 스크롤 값 `-600`, `600`은 템플릿에 적힌 숫자다. 처리기는 ±3000으로 자른다. 그대로 옮긴다.)

---

## 10. 설정 (`view/settings.rs`: 안전과 한도, 알림, 작업과 보관)

권한 모드 패널은 홈과 같은 부품이다(`view/settings.rs:23` `security_panel(…, true)` → 2장의 `consts.security.modes[]`, `snapshot.ui.security_mode`). 보조 탭은 1장의 `consts.pages`, `consts.settings_tabs`(`view/settings.rs:13`).

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `consts.routines.daily_wakes_max` | `number`. 루틴이 하루에 Lucia를 깨우는 횟수의 기본값. 입력의 placeholder "비우면 N" | `view/settings.rs:161` `crate::routines::DAILY_WAKES_MAX` |
| `consts.routines.daily_wakes_range` | `[number, number]`. 같은 입력의 `min`, `max`(1, 200) | `view/settings.rs:161` `min="1" max="200"`(`crate::routines::DAILY_WAKES_RANGE`) |
| `consts.signals.poll_minutes_range` | `[number, number]`. "확인 간격(분)"의 `min`, `max`(1, 60) | `view/settings.rs:205` `min="1" max="60"`(`crate::events::POLL_MINUTES_RANGE`) |
| `consts.signals.daily_range` | `[number, number]`. "하루 분류 상한", "하루 깨우기 상한"의 `min`, `max`(0, 500) | `view/settings.rs:209,212` `min="0" max="500"`(`crate::signals::DAILY_RANGE`) |
| `consts.wrapup.keep_days_range` | `[number, number]`. "대화 원본 보관 기간(일)"의 `min`, `max`(7, 3650; 3650은 처리기에 적힌 값) | `view/settings.rs:314` `min="7" max="3650"`(`crate::engine::wrapup::KEEP_DAYS_MIN`) |
| `snapshot.notice_rules[i].ui.rule_label` | `string`. "유용해요", "브리핑에만", "필요 없어요", 모르면 "?" | `view/settings.rs:237` `crate::attention::rule::label(&r.rule)` |
| `snapshot.ui.kept_files_total_text` | `string`. 보관 파일 크기 합계("12 KB") | `view/settings.rs:269,275` `crate::files::human_size(total)` |
| `snapshot.kept_files[i].ui.size_text` | `string`. 파일 크기 | `view/settings.rs:291` `crate::files::human_size(k.file.size)` |
| `snapshot.kept_files[i].ui.kind_label` | `string`. "이미지", "PDF", "텍스트", "Office 문서", "열 수 없는 종류" | `view/settings.rs:291` `crate::files::Kind::parse(&k.file.kind).label()` |
| `snapshot.kept_files[i].ui.direction_label` | `string`. "받은 것", "웹에서 받은 것", "만든 것" | `view/settings.rs:292` `crate::files::direction_label(&k.file.direction)` |
| `consts.dev.default_cost_cap_usd` | `number`. 개발 위임 비용 상한을 비웠을 때의 기본값(달러). 라벨의 "비우면 N"에 `3`처럼 소수점 없이 적는다(`3.5`면 `3.5`) | `view/settings.rs:354` `crate::dev::DEFAULT_COST_CAP_USD` |

`signals.recent[i].ui.verdict_label`은 활동 화면용이다(5장).

**주지 않은 것**: 없음. (위에 적은 것 말고 입력의 `maxlength`, `min`/`max`는 템플릿에 적힌 글자 그대로라 예전 값을 옮긴다. `view/settings.rs:142`의 `money(v)`는 숫자를 글로 바꾸는 것뿐이다.)

---

## 11. 시스템 (`view/system.rs`)

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `snapshot.backup.ui.cron_text` | `string \| null`. 백업 주기를 말로("매일 04:00", "매시 정각"). 흔한 꼴이 아니면 `null`이고 그때는 `backup.cron` 원문을 적는다 | `view/system.rs:100` `cron_text(&s.backup.cron).unwrap_or_else(…)` |
| `snapshot.schedules[i].ui.label` | 루틴 이름(이전부터) | `view/system.rs:148` `routine_label(&r.id)` |
| `snapshot.schedules[i].ui.cron_text` | `string \| null`. 주기를 말로. `null`이면 `spec` 원문(이전부터) | `view/system.rs:149` `cron_text(&r.spec).unwrap_or_else(…)` |
| `snapshot.session.ui.short_id` | `string`. 상주 세션 ID의 앞 8글자(`session`이 `null`이면 없다). 화면은 뒤에 "…"를 붙여 적는다 | `view/system.rs:68` `sess.id.chars().take(8).collect::<String>()` |
| `snapshot.ui.memory_bytes` | `number`. 기억 항목 전체의 크기(제목·요약·세부 정보의 UTF-8 바이트 합). `bytes()`로 적는다 | `view/system.rs:78` `s.memories.iter().map(\|e\| e.title.len() + e.summary.len() + e.body.len()).sum()` |

**주지 않은 것**: 없음. (맥락 게이지 계산은 스냅숏 값만 읽는다.)

---

## 12. 로그인 없이 여는 화면 (`view/standalone.rs`)

이 화면들은 `consts`가 없는 `Standalone` 응답(`{ kind: "standalone", screen: { type, … } }`)이다. 바깥 코드를 부른 값은 하나다.

| JSON 경로 | 타입·뜻 | 바꾼 호출 |
|---|---|---|
| `screen.attempts` (`screen.type == "site_login_form"`) | `number`. 로그인에 실패해도 같은 링크로 다시 넣을 수 있는 횟수 | `view/standalone.rs:122` `crate::browser::logins::INPUT_ATTEMPTS` |

`screen.attempts`는 `derive.rs`가 아니라 `Standalone`의 정의(`admin/spa.rs`)가 직접 싣는다. 그 밖에 `view/standalone.rs:155`의 `link.contains("/browser/handover")`는 글 검사뿐이라 화면이 한다.

---

## 13. 집계

화면 파일마다 위 표의 "값" 수(첫 칸의 JSON 경로 하나가 값 하나)다. 같은 값이 여러 화면에 쓰이면 각 화면에서 한 번씩 센다. 몇 개는 예전부터 있었다(표에 "이전부터"로 적었다).
항목 = 항목·객체의 `ui`(`extra` 안 포함), 전체 = `snapshot.ui`, 상수 = `consts`, 안 줌 = 화면에 남긴 호출, 단독 = `Standalone` 응답의 칸(`screen.*`).
"상수"에는 템플릿에 숫자가 글자로 적혀 있던 서버 한도(루틴 깨우기, 신호, 보관 기간, 이모지 크기, 화면 넘겨받기 시간)가 섞여 있다. 서버의 검사 범위와 어긋나지 않게 값을 서버에서 받는다.

| 파일 | 항목 | 전체 | 상수 | 안 줌 | 단독 |
|---|---|---|---|---|---|
| `ui.rs (공통 틀)` | 2 | 0 | 2 | 4 | 0 |
| `home.rs` | 12 | 1 | 1 | 0 | 0 |
| `work.rs` | 10 | 0 | 0 | 0 | 0 |
| `automation.rs` | 16 | 1 | 7 | 0 | 0 |
| `activity.rs` | 9 | 0 | 1 | 0 | 0 |
| `lucia.rs` | 19 | 0 | 1 | 2 | 0 |
| `expressions.rs` | 4 | 0 | 3 | 1 | 0 |
| `connections.rs` | 17 | 0 | 6 | 0 | 0 |
| `browser.rs` | 3 | 0 | 2 | 0 | 0 |
| `settings.rs` | 4 | 1 | 6 | 0 | 0 |
| `system.rs` | 4 | 1 | 0 | 0 | 0 |
| `standalone.rs` | 0 | 0 | 0 | 0 | 1 |
| **합계** | 100 | 4 | 29 | 7 | 1 |

---

## 14. 초기 설정과 프로필 이름 (새 화면, 결정 0064)

예전 화면 코드에 없던 화면이라 "바꾼 호출" 칸이 없다(그래서 위 13장의 집계에도 넣지 않았다). 값은 `admin/setup.rs`(단계, 환경)와 `admin/derive.rs`·`admin/spa.rs`(이름, 상수)가 싣는다. 화면은 `src/pages/Start.svelte`다.

### 화면 전체의 이름 `name`

| JSON 경로 | 타입·뜻 |
|---|---|
| `name` (모든 `kind: "page"` 응답과 모든 `kind: "standalone"` 응답의 맨 위 칸) | `string`. 관리 화면과 앱에 보이는 이름. 정하지 않았으면 기본 이름(`consts.profile.default_name`, `"Lucia"`). 쓰는 곳: 틀의 머리글(`Shell`의 "<이름> 관리"), 본문 맺음("<이름> · 관리 페이지"), 로그인 없는 화면의 카드 머리(`Solo`), 탭 제목("<화면 이름> · <이름>", `Page`·`Solo`). |

- 화면 코드는 이름을 계산하지 않는다. 부품은 `data.name`이나 `siteName()`(`src/lib/app.svelte.ts`: 지금 화면의 `name`, 답을 받기 전에는 기본 이름)으로 읽는다.
- 로그인한 화면의 `name`은 `spa::page`가 `snapshot.profile.name`에서 싣고, 로그인 없는 화면의 `name`은 `spa::layer`가 처리기가 끝난 뒤에 싣는다(처리기 28곳이 저마다 이름을 읽지 않아도 된다. 새 로그인 없는 화면을 더해도 저절로 실린다).
- **이름이 닿는 곳은 여기까지다**(앱 API의 `profile.name`은 `docs/app-api.md`). Lucia의 말투·부르는 이름(캐릭터), 모델에 가는 글, 도구 설명, Discord·푸시로 가는 알림 글의 "Lucia"는 그대로다.

### 프로필 `snapshot.profile`

| JSON 경로 | 타입·뜻 |
|---|---|
| `snapshot.profile.name` | `string`. 보이는 이름(위 `name`과 같다) |
| `snapshot.profile.name_set` | `boolean`. 이름을 직접 정했나. 거짓이면 입력 칸을 비워 두고 `placeholder`로 기본 이름을 보인다 |
| `snapshot.profile.avatar` | `null`(정한 그림 없음) 또는 `{ id, mime, width, height, size, set_at, by, ui }`. `by`는 `"lucia"`(Lucia의 도구) · `"owner"`(관리 페이지) · `"user"`(명령) |
| `snapshot.profile.avatar.ui.by_label` | `string`. "Lucia가 정함", "내가 정함" |
| `snapshot.profile.avatar.ui.facts` | `string`. "800×800 · PNG · 120.6 KB" |
| `consts.profile.default_name` | `string`. 기본 이름 |
| `consts.profile.name_max_chars` | `number`. 이름의 글자 수 상한(입력의 `maxlength`) |
| `consts.profile.avatar_rules` | `string`. 그림 규칙 한 줄("움직이지 않는 PNG·JPEG, 정사각형, 한 변 128~2048px, 3.0 MB까지") |

- 그림은 `/profile/avatar/file/{id}`로 본다(`<Picture src>`). 로그인한 세션만 받고, 화면 전환으로 가로채지 않는 주소다(`isRaw`). `id`가 내용 해시라 바뀌지 않는다.
- 이름은 `/profile/name`(`csrf`, `name`), 그림은 `/profile/avatar`(multipart `csrf`, `file`)와 `/profile/avatar/remove`로 바꾼다.

### 초기 설정 `snapshot.setup`

| JSON 경로 | 타입·뜻 |
|---|---|
| `snapshot.setup.complete` | `boolean`. **필수를 마쳤나**. 필수는 대화할 곳 하나뿐이다: Discord 봇 토큰과 소유자가 모두 있거나, 알림 받을 곳이 앱이다. 로그인 뒤 첫 화면을 고를 때도 같은 규칙을 쓴다(`admin::setup::channel_ready`) |
| `snapshot.setup.dismissed` | `boolean`. 홈의 안내를 껐나(설정 `setup.dismissed`). 홈의 안내 상자는 `!complete && !dismissed`일 때만 보인다 |
| `snapshot.setup.config_path` | `string \| null`. 데몬이 읽어 온 설정 파일의 경로(모르면 `null`). 읽기만 한다 |
| `snapshot.setup.steps[]` | `{ key, title, icon, state, state_label, tone, summary }[]`, 순서대로 `profile`(이름과 모습) · `discord` · `google` · `app`(앱과 푸시) · `images` · `env`(환경). 구역의 `id`는 `key`다 |
| `steps[i].state` | `"done" \| "todo" \| "optional"`. `done`: 끝남. `todo`: 대화할 곳이 아직 없을 때 그 길이 될 수 있는 단계(`discord`, `app`). `optional`: 그 밖. 환경은 점검이 모두 정상이면 `done` |
| `steps[i].state_label`, `.tone` | 배지의 글("완료", "필요", "선택")과 결(`ok`, `warn`, `muted`) |
| `steps[i].summary` | `string`. 데몬이 계산한 한 줄 요약("봇 … 연결됨 · 소유자 설정됨", "앱 토큰 1개 · 알림 받는 곳 앱 모두 · 푸시 꺼짐"(알림을 앱으로 받으면 늘 "앱 모두", 아니면 "Discord DM", 결정 0067), "점검 5개 모두 정상입니다." …). 화면은 그대로 적는다 |
| `steps[i].icon` | `string`. 구역의 아이콘 이름 |
| `snapshot.setup.env[]` | `{ key, label, tone, detail, command }[]`. 환경 단계의 줄(설정 파일, 상주·브라우저·스크립트 샌드박스, 접속 API, 실행 환경 점검). `detail`은 상태, `command`는 사용자가 터미널에서 돌릴 명령(`null`이면 없음). 데몬은 샌드박스를 만들지도 설정 파일을 고치지도 않는다 |

- 단계의 폼이 필요로 하는 사실(Discord 토큰·소유자 있음, Google 클라이언트·동의, 앱 목록, FCM, 이미지 키)은 연결 화면과 같은 스냅숏 칸(`snapshot.discord`, `.google`, `.clients`, `.app`, `.images`, `.gateway_binds`)을 그대로 읽는다. 비밀 값은 실리지 않는다(있는지만 알린다).
- 이 화면의 폼은 연결 화면의 처리기를 같은 주소·같은 칸 이름으로 부른다. 처리기는 늘 연결 화면으로 돌려보내므로 `ActionForm`·`PostButton`·`Confirm`의 `stay`로 이 화면에 남는다(`pages.md` 5장).
- 안내 끄기·켜기: `/start/dismiss`(홈으로), `/start/reopen`(이 화면으로).
