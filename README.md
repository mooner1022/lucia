# Lucia

홈서버에서 늘 켜 두는 **한 사람용 개인 비서 에이전트**다. Claude Code를 실행 엔진으로 쓰고, 그 바깥의 일(기억, 일정, 상태, 알림, 승인)은 Rust 데몬 `luciad`가 맡는다.

- 메신저(Discord DM이나 자체 앱)로 말을 걸면 답하고, 맡긴 일을 기억했다가 스스로 챙긴다.
- 아침 브리핑, 메일·일정 변화 알림, 약속한 후속 조치, 반복 루틴, 페이지 감시를 사람 없이 돈다.
- 메일을 보내거나 폼을 제출하는 것처럼 바깥에 남는 일은 **승인 카드**로 먼저 묻는다.
- 에이전트는 가상 머신(Docker Sandboxes) 안에서만 돌고, 계정과 자료에 닿는 일은 호스트의 데몬이 확인하고 대신 한다.

## 빠르게 올리기

준비물: KVM이 되는 리눅스 서버, [Docker Sandboxes(`sbx`)](https://docs.docker.com/ai/sandboxes/), Claude 구독, Rust 1.93 이상, Node.js(빌드에만).

```sh
git clone https://github.com/mooner1022/lucia.git ~/lucia && cd ~/lucia

# 1. sbx 준비(한 번): Docker 로그인, 전역 정책, Claude 로그인
sbx login
sbx policy init deny-all
#    Claude 로그인은 임시 샌드박스에서 /login 한 번(docs/DEPLOY.md 2장)

# 2. 설정 파일
mkdir -p ~/.config/lucia && cp deploy/lucia.example.toml ~/.config/lucia/lucia.toml

# 3. Lucia가 사는 샌드박스
deploy/create-sandbox.sh

# 4. 빌드, 설치, 서비스 시작
deploy/install.sh
~/.local/lib/lucia/luciad check          # 모두 ✓인지

# 5. 관리 페이지 비밀번호 링크(터미널에 찍힌다)
~/.local/lib/lucia/luciad admin-setup
```

링크에서 비밀번호를 정하고 로그인하면 **초기 설정** 화면이 뜬다. 여기서 이름과 프로필 그림, Discord 봇, Google 메일·일정, 앱 푸시(Firebase), 이미지 생성 키를 넣는다. 필수는 대화할 곳 하나(Discord 봇이나 앱)뿐이다.

단계마다의 설명, 선택 기능(브라우저, 코드 실행, 앱 접속 API, 밖에서 접속), 업데이트와 되돌리기, 막혔을 때 볼 것은 **[배포 가이드](docs/DEPLOY.md)**에 있다.

## 구성

| 부분 | 무엇 |
|---|---|
| `crates/luciad` | 데몬: 채널, 스케줄러, 작업 큐, 저장소(SQLite), MCP 서버, 승인 데스크, 브라우저 프록시, 관리 페이지, 앱 접속 API |
| `crates/lucia-*` | 공통 타입, 채널 어댑터(Discord), 실행 계층(Claude Code), 저장소, 사전 체크 |
| `web/admin` | 관리 화면(Svelte, 빌드해서 데몬 바이너리에 넣는다) |
| `plugin/` | 샌드박스에 읽기 전용으로 붙는 훅(허용 도구 이중 방어) |
| `character/character.md` | 캐릭터(이름, 말투, 관계). 관리 페이지 "캐릭터"에서 고친다 |
| `deploy/` | 설치 스크립트, 샌드박스 만들기, 서비스 유닛, 설정 예시 |

샌드박스는 셋까지 쓴다: 상주 대화(`lucia`), 로그인 프로필을 가진 브라우저(`lucia-browser`, 선택), Lucia가 쓴 코드를 돌리는 곳(`lucia-script`, 선택).

## 문서

- [배포 가이드](docs/DEPLOY.md): 처음 올리기부터 업데이트까지
- [구현 현황](docs/STATUS.md): 무엇을 할 수 있고 누가 어디까지 할 수 있는지(기능과 권한)
- [앱 접속 API](docs/app-api.md): 자체 앱이 붙는 WebSocket·HTTP 규격
- [결정 기록](docs/decisions/): 설계를 그렇게 한 까닭
- [개발 계획](docs/PLAN.md), [PoC 결과](poc/RESULTS.md)

## 개발

```sh
cargo test --workspace                     # 모델을 부르지 않는다
deploy/build-ui.sh                         # 관리 화면: 의존성, 타입 검사, 시험, 빌드
web/admin/e2e/run.sh --sets stress,full,alt --shots   # 실제 브라우저로 배치 검사
```

개발할 때의 규칙(보호 영역, 인증 정보 다루기, 관리 화면 짜는 법)은 [`CLAUDE.md`](CLAUDE.md)에 있다. 문서와 주석은 한국어로 쓴다.

## 알아 둘 것

- 소유자 한 명을 위한 구성이다. 여러 사용자를 나누는 기능은 없다.
- 대화와 정기 실행이 Claude 구독 사용량을 쓴다.
- 관리 페이지와 접속 API는 집 LAN·VPN 주소에만 연다. 밖으로 열 때는 TLS를 맡는 리버스 프록시와 2단계 인증을 쓴다.
