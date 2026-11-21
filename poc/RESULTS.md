# PoC 결과

1단계(PoC — 실행 계층 검증) 실험 기록. 실험 정의와 통과 기준은 [docs/PLAN.md §5](../docs/PLAN.md#5-1단계--poc-실행-계층-검증)를 따른다.

각 실험마다 날짜, 환경(sbx 버전, VM 안 Claude Code 버전), 실행한 명령, 출력 요약, 결론, 후속 조치를 적는다. 인증 정보나 토큰이 들어간 출력은 남기지 않는다.

| # | 실험 | 상태 |
|---|---|---|
| P1 | sbx 호스트 측 Claude 로그인 | **통과**(데몬 전용 사용자·Docker 로그인 만료는 MVP 배포로 넘김) |
| P2 | `sbx run claude -- -p`와 stream-json, 권한 모드 상호작용 | **통과**(permission-prompt-tool 부분은 P6으로) |
| P3 | 샌드박스 백그라운드 유지와 재부팅 복구 | **부분 확인, 재부팅 시험 건너뜀**(호스트에 다른 서비스가 있어 재부팅 불가, 아래 참고) |
| P4 | VM → 호스트 MCP 접근과 네트워크 정책 | **통과** |
| P5 | 세션 재개 | **통과**(샌드박스 안, 대조군 포함) |
| P6 | 권한 승인 도구 왕복 | **통과** |
| P7 | VM 안 Claude Code 버전과 플래그 확인 | **통과**(API 키 백엔드 전환은 키가 없어 미시험) |
| P8 | 브라우저 샌드박스: 출구 IP, TLS, CDP·noVNC 노출(4단계 첫 실험) | **통과**(조건: 세션을 붙잡아 둬야 샌드박스가 멈추지 않는다) |

## P8b — 브라우저 출구를 차단 목록으로 (2026-11-17, sbx만으로는 안 됨 → 데몬 프록시)

결정 0008의 근거. 개발 샌드박스 `lucia-browser-dev`(shell 킷)에서 했다.

- **sbx 정책으로 차단 목록**(`**` 허용 + 사설 대역 CIDR·내부 이름 거부, 거부가 이긴다):
  - IP를 직접 적은 주소(`http://192.168.0.1/`)는 막힌다.
  - **이름으로 풀리는 사설 IP는 막히지 않았다**: `192.168.0.250.nip.io`, `192.168.1.1.nip.io`에는 sbx 프록시가 실제로 연결을 시도했다("upstream request … failed"). `172.16.5.5.nip.io`, `10.1.2.3.nip.io`도 시도했다(시간 초과).
  - 막힌 것은 이 호스트 자신의 주소(브리지·LAN)로 풀리는 이름뿐이었다.
  - 그래서 `**`를 열면 LAN 기기에 이름만으로 닿는다. 쓰지 않는다.
- **VM → 호스트 포트는 원시 TCP가 통한다**: 샌드박스 범위로 `localhost:<포트>`를 허용하면 `host.docker.internal:<포트>`로 HTTP가 아닌 바이트, GET, CONNECT 모두 그대로 호스트 루프백에 닿았다.
  - 그래서 데몬이 SOCKS5 프록시를 열고 Chrome을 `--proxy-server=socks5://…`로 띄운다.
  - 기본 거부 정책에서 VM의 직접 연결은 이름 풀기부터 실패한다(`curl` rc=6). sbx 프록시(`gateway.docker.internal:3128`)로 가면 403이다.
- **데몬 프록시로 확인**(`luciad browser-check`, 모두 통과):
  - 퀘이사존 첫 페이지를 Chrome이 프록시로 연다(Cloudflare 통과).
  - 시험 페이지의 `<img src=http://10.1.2.3.nip.io/…>`와 `192.168.0.250.nip.io`로 넘기는 302를 프록시가 막는다.
  - Chrome은 HTTPS-First라 443을 먼저 시도하고, 80도 막힌다.
- **운영 전환**(2026-11-17 02:06 KST, `deploy/create-browser-sandbox.sh --policy-only`):
  - 예전 퀘이사존 허용을 지우고 `localhost:47811`만 허용했다.
  - VM 안 `curl --socks5-hostname`으로 확인했다:
    - naver 200, 퀘이사존 403(curl에 대한 Cloudflare, 연결은 됨).
    - 막힘: `<LAN 기기 IP>.nip.io`, 관리 페이지 `<호스트 LAN IP>:8780`(포트), 직접 연결(rc=6), sbx 프록시 경유(403).
  - `lucia` 샌드박스는 `localhost:47811`이 거부다.
  - 직접 연결(프록시를 타지 않는 Chrome 예외·WebRTC를 가정):
    - IP를 적은 TCP는 공인 1.1.1.1(rc=35), 공유기·LAN 기기·10/8·172.17.0.1(rc=52)이 sbx에서 끊겼다. 169.254.169.254는 rc=7이다.
    - UDP 53번(1.1.1.1, 8.8.8.8, 공유기)은 답이 없다.
- **MCP 이미지 결과**: `claude -p`(2.1.325)는 MCP 도구가 돌려준 `image` 블록을 그대로 `tool_result`에 넣어 모델에게 준다(기록에 `image/jpeg`). 모델은 글에 없는 로고 색·메뉴 위치를 맞게 설명했다.

## P8 — 브라우저 샌드박스 (2026-11-16, 통과)

4단계 착수 첫 실험([PLAN §7](../docs/PLAN.md#7-37단계-개요)). 버리는 샌드박스(`shell` 킷, 메모리 3 GiB, 작업 공간 없음)에서 했다. 대상은 퀘이사존(사용자 선택)이고, 로그인하지 않고 페이지는 두 번(메인, 로그인)만 열었다. sbx v0.51.0, Chromium 156(Debian 13 패키지).

### 결론

- **간다.** 실제 Chrome이 집 IP로, 프록시의 TLS 가로채기 없이 퀘이사존(Cloudflare 뒤)을 챌린지 없이 연다. CDP와 noVNC 모두 호스트 루프백으로 꺼낼 수 있다.
- 조건 둘:
  - **세션을 붙잡아 둬야 한다.** `sbx exec`가 하나도 없으면 샌드박스가 1분쯤 뒤 스스로 멈추고 Chrome도 같이 끝난다. 데몬이 오래 도는 `sbx exec … sleep infinity` 세션을 하나 붙잡고, 끊기면 다시 띄운다.
  - **포트 게시는 VM 루프백에 닿지 않는다.** VM 안에 중계(socat)를 둔다.
- 데몬 설계에 반영: CDP는 포트를 열지 않고 붙잡아 두는 `sbx exec -i` 세션의 표준 입출력으로 잇는다(아래 "CDP를 포트 없이"). 쿠키는 정상 종료·샌드박스 정지·재시작 뒤에도 남는다.

### 네트워크

- 샌드박스 기본 정책은 전부 거부다(`shell` 킷은 허용 규칙을 더하지 않는다). 허용은 샌드박스 범위 규칙(`sbx policy allow network --sandbox <이름> …`)으로 한다. `--sandbox`를 빼면 모든 샌드박스(상주 `lucia` 포함)에 적용되므로 반드시 붙인다. 와일드카드(`*.quasarzone.com`, `**.example.com`)를 쓸 수 있다.
- 나가는 길은 `HTTPS_PROXY=http://gateway.docker.internal:3128`이다. VM에는 "Docker Sandboxes Proxy CA"가 설치돼 있다.
- **TLS를 가로채지 않는다(허용된 호스트).** VM 안에서 본 quasarzone.com 인증서 발급자가 호스트와 같다(Google Trust Services). 사이트는 Chrome의 TLS를 그대로 본다.
- 막힌 호스트에는 프록시가 자기 인증서로 답한다. Chrome은 그 CA를 믿지 않으므로 `ERR_CERT_AUTHORITY_INVALID`로 끝난다(막힌 것은 그대로 막힌다). Chrome에 프록시 CA를 넣지 않는다. 넣지 않아야 혹시 가로채는 연결도 거부된다.
- **출구 IP는 호스트와 같다(집 IP).** VM과 호스트에서 같은 IP 확인 서비스로 비교했다. 주소는 기록하지 않는다.
- curl은 VM과 호스트 모두 403(Cloudflare 봇 차단)이었고, 실제 Chrome은 200이었다.

### Chrome

- 화면 있는 Chromium을 가상 화면(Xvfb :99)에 띄운다. 사용자 에이전트는 보통 Chrome이다(HeadlessChrome이 아니다). 전용 프로필 디렉터리를 쓰고 `--proxy-server`로 프록시를 지정한다.
- 퀘이사존 메인과 로그인 페이지: 200, 정상 렌더링(한글 글꼴 포함), 챌린지 없음.
- 정책에 막힌 요청은 광고·추적·통계였다(naver wcs, googletagmanager, doubleclick, clarity, cloudflareinsights, contentsfeed, anymind). 사이트 동작에는 퀘이사존 호스트만 필요해 보인다. `cdnjs.cloudflare.com`도 막혔는데, 메인 표시에는 문제가 없었다. 로그인 동작에 필요한지는 로그인할 때 본다.
- 로그인 페이지: 아이디(`login_id`, text)·비밀번호(`password`, password) 칸, "자동 로그인" 체크, 보이는 캡차 없음. 간편 로그인(네이버·카카오·구글·애플)도 있다.
- `/tmp`와 프로필은 샌드박스가 멈췄다 켜져도 남는다. 그래서 프로세스가 없으면 남은 잠금 파일(`/tmp/.X99-lock`, 프로필의 `Singleton*`)을 지우고 띄워야 한다. 지우지 않으면 Xvfb가 뜨지 않는다.
- 메모리: VM 사용 약 600 MB(Chromium 프로세스 RSS 합 약 1.3 GB, 공유 메모리 중복 포함).

### CDP와 noVNC

- `sbx ports <샌드박스> --publish 127.0.0.1:<호스트>:<샌드박스>`는 VM 루프백에서만 듣는 서비스에 닿지 않는다(Chrome CDP가 그렇다). VM 안에 `socat TCP-LISTEN:9223,fork TCP:127.0.0.1:9222`를 두고 9223을 게시하면 호스트에서 `/json/version`과 웹소켓 CDP가 된다.
- noVNC: `x11vnc -display :99 -localhost`(VM 루프백 5900) + `websockify --web /usr/share/novnc 0.0.0.0:6080 127.0.0.1:5900`. 게시하면 호스트에서 `vnc.html` 200, `/websockify` 웹소켓으로 `RFB 003.008`까지 왔다.
  - x11vnc는 샌드박스 환경의 `WAYLAND_DISPLAY`를 보고 "Wayland"라며 끝난다. 이 변수를 빼고 띄운다.
- 게시한 포트 설정은 샌드박스가 멈춰도 남고, 다시 켜지면 다시 묶인다.
- 격리: 상주 `lucia`에서 CDP·noVNC 호스트 포트는 거부, 브라우저 샌드박스에서 데몬 포트(MCP, 관리 페이지)와 `api.anthropic.com`·`github.com`은 거부다(`sbx policy check`). 저장된 시크릿은 전역 `anthropic` 하나다. 브라우저 샌드박스에는 Anthropic 쪽 호스트를 명시 거부 규칙으로도 막는다(나중에 넓은 허용이 생겨도 열리지 않게).
- CDP에는 인증이 없다(쿠키, 비밀번호 칸까지 전부 다룬다). 호스트 루프백에만 두고, 사람이 볼 noVNC는 관리 페이지 로그인 뒤에서만 중계한다.

### CDP를 포트 없이: `sbx exec -i` 표준 입출력

- `sbx exec -i <샌드박스> cat`에 1 MiB(0~255 모든 바이트)를 흘려 그대로 돌아왔다(TTY 없이는 바이너리 그대로).
- 그래서 붙잡아 두는 세션 자체를 CDP 통로로 쓸 수 있다: VM 안에서 브라우저를 띄우고 `socat STDIO TCP:127.0.0.1:9222`로 잇는다. 호스트에 CDP 포트를 열지 않으므로 이 서버의 다른 프로세스가 로그인된 브라우저를 조종할 수 없다. 데몬이 연결을 닫으면 세션이 끝나고, 샌드박스는 1분쯤 뒤 스스로 멈춘다(쓰지 않을 때 메모리를 돌려준다).

### 쿠키 유지

- 메인을 한 번 열어 생긴 퀘이사존 쿠키 4개(모두 만료 시각이 있는 쿠키)를 확인했다. 그 뒤 `Browser.close`로 Chrome을 정상 종료하고, 세션을 풀어 샌드박스가 멈추게 한 다음 다시 띄웠다. 페이지를 열지 않고도 4개가 그대로 있었다(값은 보지 않았다).
- 사이트 세션 쿠키(`quasarzone_session`)는 약 5시간짜리다. 로그인을 오래 유지하려면 로그인 때 "자동 로그인"을 켜야 할 것으로 보인다. 첫 실제 로그인 때 확인한다.

### 설치

- apt 원본은 `deb.debian.org`, `dhi.io`(80)다. 설치하는 동안만 샌드박스 범위로 허용하고 끝나면 지운다. 패키지: `chromium xvfb x11vnc novnc python3-websockify fonts-noto-cjk`(`--no-install-recommends`). 디스크 약 0.6 GB.
- 정책 규칙을 지울 때 비대화형이면 `sbx policy rm network --sandbox <이름> --id <규칙 ID> --force`. 규칙 ID는 `sbx policy ls <이름> --wide`에 나온다.

## P3 — 샌드박스 유지와 재부팅 복구 (2026-11-16, 재부팅 시험 건너뜀)

### 현재 상태: 재부팅 시험을 건너뛰었다

- **결정(2026-11-16, 사용자)**: 이 호스트에는 다른 서비스도 함께 돌고 있어 시험을 위해 재부팅하기 어렵다. 재부팅 시험은 건너뛰고 다음 단계로 진행한다.
- **확인한 것**: 샌드박스를 계속 띄워 둘 필요가 없다는 것, 자동 정지와 재시작 뒤 세션이 이어진다는 것, 데몬 재시작(아래).
- **확인하지 못한 것(위험으로 남음)**: 재부팅 뒤 sbx 데몬이 무인으로 다시 뜨는지, Docker 로그인과 Claude OAuth를 재부팅 뒤에도 쓸 수 있는지, 메모리에만 있던 상태가 모두 사라진 뒤 복구되는지. 특히 로그인 정보는 키링에 기대고 있을 가능성이 있어, 재부팅 뒤 비서가 조용히 멈출 수 있다(아래 "재부팅해야만 알 수 있는 것").
- **완화(MVP에서 반영)**:
  - 데몬 시작 때마다 `sbx diagnose`(데몬, 가상화, 인증)와 짧은 `claude -p` 점검을 돌리고, 실패하면 Discord로 알린다(명세 원칙 8). 인증 오류는 `is_error`·`terminal_reason`으로 감지한다(P1).
  - 부팅 시 자동 시작은 MVP 배포 단계에서 systemd 유닛(kvm 그룹, linger 또는 데몬 전용 사용자)으로 만든다.
- **언제 다시 시험하나**: 계획된 유지보수 등으로 호스트를 재부팅하게 될 때. 재부팅 뒤 kvm 그룹이 있는 로그인 셸에서 `poc/p3/after_reboot.sh`를 돌린다(재부팅 전 시험 실행은 통과). 그 전에 `lucia-poc` 샌드박스를 지우면 스크립트의 세션 ID를 새로 만든 세션으로 바꿔야 한다.

### 재부팅 전에 확인한 것

- **샌드박스는 계속 띄워 둘 필요가 없다.** sbx 데몬은 마지막 세션(exec)이 끝나고 30초 뒤 샌드박스를 자동으로 멈춘다(데몬 로그: `auto-stop grace period`, delay 30s. 바꾸는 설정은 찾지 못했다). `sbx exec`는 멈춘 샌드박스를 먼저 시작한다. 멈춘 상태에서 exec까지 약 2.0초, 실행 중이면 0.14초.
- **멈췄다 다시 시작해도 세션이 이어진다.** P4의 세션(`<세션 ID>`)을 자동 정지 뒤 재개해 기억한 단어를 정확히 답했다. 세션 기록은 VM 안 `~/.claude/projects/<작업 공간 경로>/`에 있고, `sbx rm`을 하면 사라진다 → 세션 기록의 VM 밖 백업(명세 미결정 사항)은 MVP에서 `sbx cp`나 작업 공간 쪽 복사로 정한다.
- **재부팅 뒤 데몬**: sbx 패키지에는 systemd 유닛이 없다. 데몬은 처음 호출한 sbx CLI가 띄우고 **그 프로세스의 그룹을 물려받는다**(P1). 운영에서는 luciad의 systemd 유닛이 kvm 그룹을 가진 채 sbx를 처음 부르거나, sbx 데몬을 따로 유닛으로 띄운다(MVP 배포에서 정한다).

### 재부팅해야만 알 수 있는 것(미확인)

- **부팅 때 누가 데몬을 띄우나**: 데몬을 돌리는 사용자의 `Linger=no`라 로그인 전에는 사용자 systemd 유닛이 돌지 않는다. 무인 복구에는 linger(`loginctl enable-linger`)나 시스템 서비스(데몬 전용 사용자)가 필요하다.
- **로그인 정보가 재부팅 뒤에도 쓰이나**: sbx는 Docker 로그인과 Claude OAuth를 `~/.config/com.docker.sandboxes/` 아래 암호화 파일(`.posixage` 잠금 파일로 보아 age 형식)로 둔다. 한편 15:09부터 `gnome-keyring-daemon --components=secrets`가 떠 있고 `~/.local/share/keyrings/`는 비어 있다. 복호화 키가 메모리나 로그인 때 풀리는 키링에 있다면, 재부팅 뒤 아무도 로그인하지 않은 상태에서는 로그인 정보를 못 쓸 수 있다(조용한 실패 위험).
- 그 밖에 메모리에만 있는 것(VM 프로세스, virtiofs 공유, 프록시, /tmp, /run의 소켓)이 모두 사라진 상태에서 처음부터 복구되는지.

### 재부팅하게 되면

kvm 그룹이 있는 로그인 셸에서 `poc/p3/after_reboot.sh`. 데몬 상태, 진단, 데몬 GID, 샌드박스 목록, Claude 로그인 시크릿을 보여 주고, 같은 세션을 재개해 단어를 확인한다(재부팅 전 시험 실행: 통과, 2.2초). 재부팅 뒤 아무도 로그인하지 않은 상태에서도 되는지는 MVP 배포의 systemd 유닛이 생긴 뒤에야 의미 있게 확인할 수 있다.

데몬 전용 시스템 사용자와 Docker 로그인 만료 시 실패 형태는 MVP 배포 단계로 넘긴다(sudo와 의도적 로그아웃이 필요하다).

## P6 — 권한 승인 도구 왕복 (2026-11-16, 통과)

명령: `luciad poc-approval --sandbox lucia-poc --workdir ~/.local/share/lucia/poc/workspace --mcp-bind 127.0.0.1:47801 --model haiku --record-dir poc/raw/p6`

실행 조합: P4와 같고, `--tools Bash --permission-prompts host --permission-prompt-tool mcp__lucia__request_approval`, VM 환경 변수 `CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS=0`을 더했다. 같은 요청(Bash로 `/tmp/lucia-p6-marker` touch)에 데몬이 다르게 답했다. PoC에서는 데몬 안의 고정 응답이 사람 역할을 했다(3단계에서 Discord 버튼으로 바꾼다).

| 경우 | 데몬 응답 | 표식 파일 | `permission_denials` | 걸린 시간 |
|---|---|---|---|---|
| 허용 | `{"behavior":"allow"}` | 생김 | 0 | 5.1초 |
| 거부 | `{"behavior":"deny","message":…}` | 없음 | 1 | 5.4초 |
| 무응답 | 15초 기다린 뒤 거부 | 없음 | 1 | 20.7초 |

- `--restricted`에서도 permission-prompt-tool이 불린다(`--permission-prompts host`일 때).
- 승인 도구가 받는 인자(문서에 없음, 실제 값): `tool_name`, `input`(Bash면 `command`와 모델이 쓴 `description`), `tool_use_id`. 승인 요청도 실행별 토큰으로 오므로 **어느 실행의 어느 도구 호출인지**(`RunId` + `tool_use_id`) 정확히 짚을 수 있다. 명세의 "그 작업의 그 행동 하나에만 유효"를 이 두 값으로 구현한다.
- `description`은 모델이 쓴 글이다. Discord 승인 메시지에는 `command` 같은 실제 입력을 보여 주고, 설명은 참고로만 붙인다.
- 거부 사유는 모델에게 전달되어 결과 보고에 그대로 반영됐다.
- 시간 제한: HTTP MCP 요청은 60초에 끊기고, 2분 넘은 MCP 호출은 백그라운드로 넘어가 `-p`에서 결과를 잃는다(문서). 승인을 사람이 오래 기다려야 하면 MCP 호출 안에서 기다리지 말고, 거부로 끝낸 뒤 "승인 대기" 작업 상태로 옮겨 승인 후 다시 실행하는 쪽이 맞다(3단계 설계 입력).
- 회귀 테스트: `mcp::tests::approval_decision_text_matches_prompt_tool_contract`.
- **확인하지 않은 범위**: P2 c1의 거부 사유는 "작업 디렉터리 밖 경로"였다. 즉 기본 모드에서도 승인 없이 실행되는 Bash 명령이 있을 수 있다. 3단계에서 승인 프롬프트에 기대기 전에 두 경우를 시험한다: 작업 공간 안 파일 쓰기, 허용된 호스트로의 POST.
- **3단계 설계 메모**: `request_approval`은 보통의 MCP 도구라 모델이 직접 부를 수도 있다. 이 도구의 응답은 **D7의 승인 레코드로 세지 않는다.** 외부 쓰기 도구의 승인은 데몬이 사람에게서 직접 받은 레코드로만 확인한다. 또 결과 보고만 하는 실행에서는 `--allowedTools`에 이 도구를 넣지 않는다.

## P7 — VM 안 Claude Code와 플래그 (2026-11-16, 통과)

VM 안 Claude Code는 2.1.325(호스트와 같다). 시험 도구: `poc/p7/run_flags.sh`. 모든 경우 데몬 실행 조합(`--restricted --strict-mcp-config --permission-prompts none`, haiku)으로 돌렸다.

| 플래그 | 결과 |
|---|---|
| `--max-turns 1` | 도움말에 없지만 동작한다. 도구를 써야 하는 요청이 `subtype=error_max_turns`, `terminal_reason=max_turns`, `is_error=true`, 종료 코드 1로 끝났다(`num_turns`는 2로 보고) |
| `--json-schema` | `StructuredOutput` 도구가 생기고, 결과는 `result.structured_output`(`{"answer":4}`)과 `result.result`(JSON 문자열)에 온다. MCP `report_result`의 2차 수단으로 쓸 수 있다 |
| `--max-budget-usd` | 구독 인증에서도 동작한다: `subtype=error_max_budget_usd`, `terminal_reason=budget_exhausted`, 종료 코드 1. **턴이 끝난 뒤에** 확인하므로 한 턴 비용은 상한을 넘을 수 있다 |
| `--append-system-prompt-file` | `--restricted`에서도 동작한다. 읽기 전용 마운트의 파일(`/opt/lucia/p2/p7-append.md`)의 지시를 따랐다 |
| `--system-prompt-snapshot off` | 받아들인다 |
| auto-memory | `--restricted` 실행의 init에는 `memory_paths`가 없다(환경 변수 없이도). `--setting-sources project` 실행에는 있었다. 데몬은 둘 다 쓴다(`--restricted` + `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`) |
| 킷 `CLAUDE.md` | `--restricted` 실행의 컨텍스트 약 3,900 토큰으로 보아 읽히지 않는다 |

- 오류 종료는 모두 `subtype`(`error_*`)과 `terminal_reason`으로 구분된다. 로그인 오류만 `subtype=success`에 `is_error=true`였다(P1). 데몬은 `is_error`를 먼저 보고 `terminal_reason`으로 나눈다.
- init에 내장 plugin 5개(`cc-plugin-*`)와 스킬 16~19개, 슬래시 명령 47~54개가 보인다. 맥락 크기에 들어가므로 MVP에서 줄일 수 있는지 본다.
- 미시험: API 키 백엔드로 바꿔 실행(D6) — 시험용 키가 없다. 키가 생기면 `sbx secret set anthropic`으로 확인한다.

## P4·P5 — VM에서 호스트 MCP, 세션 재개 (2026-11-16, 통과)

**PoC 완료 기준(명세 §로드맵 1)을 샌드박스 안에서 충족했다**: 데몬이 같은 세션에 두 번 이어 보낸 요청에서 첫 요청의 맥락이 유지되고, 구조화된 결과 보고를 받았다.

명령:

```sh
sbx policy allow network localhost:47801 --sandbox lucia-poc   # 이 샌드박스만 호스트 포트 허용
luciad poc --sandbox lucia-poc --workdir ~/.local/share/lucia/poc/workspace \
  --mcp-bind 127.0.0.1:47801 --model haiku --record-dir poc/raw/p4
```

실행 조합(`luciad poc`가 만든 것): `sbx exec -i -w <작업 공간> -e CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 lucia-poc claude -p --input-format stream-json --output-format stream-json --verbose --restricted --session-id|--resume <uuid> --model haiku --mcp-config '<JSON 문자열>' --strict-mcp-config --permission-prompts none --tools "" --allowedTools mcp__lucia__report_result`.

| 확인 | 결과 |
|---|---|
| VM → 호스트 MCP | `http://host.docker.internal:47801/mcp`로 연결(`connected`). 데몬은 127.0.0.1에만 바인드하고, rmcp `allowed_hosts`에 `host.docker.internal`을 더했다 |
| 실행별 토큰 | 데몬이 실행마다 토큰을 발급해 `--mcp-config`의 `Authorization` 헤더로 넘기고, 실행이 끝나면 회수한다. 보고 3건이 모두 그 실행의 토큰으로 왔다(실행과 보고 짝짓기) |
| 토큰 없는 요청 | VM에서 토큰 없이 401, 틀린 토큰 401, 올바른 토큰은 인증 통과(400: ping 본문이라 MCP가 거절, 인증과 무관) |
| 외부 접속 | VM에서 `https://example.com` → 403(프록시 차단) |
| 세션 재개 | 요청 1·2가 같은 세션 ID, 요청 2가 단어를 정확히 보고 |
| 대조군 | 새 세션은 `needs_input / 모름` |
| `--restricted` | 도구 목록이 `["mcp__lucia__report_result"]`뿐. `--allowedTools`는 `--restricted`에서도 적용된다. 권한 모드 `default`, 거부 0건 |
| 비용·크기 | haiku 3회, API 환산 약 $0.0012. 실행당 컨텍스트 약 3,900~4,200 토큰 |

### 킷 허용 규칙 좁히기

P1에서 본 킷의 허용 규칙 8개 중 계정 쪽 엔드포인트와 패키지·업데이트 주소를 샌드박스 범위 거부 규칙으로 막았다(거부가 허용보다 우선). 프록시가 이 호스트들에 실제 토큰을 넣는지는 VM 안에서 관찰할 수 없으므로, 확인하기보다 막는 쪽을 택했다.

```sh
sbx policy deny network mcp-proxy.anthropic.com,bridge.claudeusercontent.com,claude.com,downloads.claude.ai,deb.debian.org,dhi.io --sandbox lucia-poc
```

- 남은 허용: `api.anthropic.com:443`, `platform.claude.com:443`(킷), `localhost:47801`(데몬 MCP).
- 좁힌 뒤 `luciad poc --sandbox`를 다시 돌려 모든 판정이 통과했다.
- `sbx policy log lucia-poc`로 본 실제 접속:
  - Claude Code 실행에 쓰인 것은 `api.anthropic.com`(447회)과 데몬 포트뿐이다. `platform.claude.com`은 로그인 때 1회.
  - Claude Code가 `http-intake.logs.us5.datadoghq.com:443`(로그 수집)으로 42회 시도했고 기본 차단에 막혔다. 차단 정책이 텔레메트리 유출도 막는다. 불필요한 재시도를 줄이는 공식 설정은 MVP에서 문서로 확인한다.
  - 샌드박스가 시작될 때 `deb.debian.org`, `dhi.io`에 접속을 시도한다(킷 이미지). 막아도 동작에는 문제가 없었다.
  - 킷 사용자 설정을 읽은 실행(P2 c4)은 sbx MCP 게이트웨이(`mcp-gateway.docker.internal`, 데몬 관리 별칭)와 `mcp-proxy.anthropic.com`에 접속했다. 데몬 실행 조합(`--strict-mcp-config`)에서는 접속하지 않는다.
- **샌드박스를 새로 만들 때**는 생성 옵션으로 같은 거부 규칙을 넣는다: `sbx create … --deny-network <호스트>`(반복). 상주 샌드박스 생성 명령에 넣을 것.

참고:

- 쉬고 있는 샌드박스는 멈추는 것으로 보인다. `sbx exec`가 "sandbox lucia-poc — started"를 stderr에 남기고 시작한다. 시작 지연과 자동 정지 조건은 P3에서 본다.
- MCP 설정은 파일 대신 JSON 문자열로 넘겼다. VM 안 파일이 필요 없고, 토큰은 실행 하나 동안만 유효하다(VM 안 프로세스 인자로는 보인다).
- 회귀 테스트: `luciad` 단위 테스트 `mcp::tests::requests_need_a_live_run_token`(토큰 없음·틀림·회수 후 401, VM Host 헤더 허용).

## P2 — 실행 방식, 권한 모드, 훅 (2026-11-16, 통과)

샌드박스: `sbx create --name lucia-poc --memory 4g --skills off --static-mcp none claude <작업 공간> <레포>/poc/p2:/opt/lucia/p2:ro`. 시험 도구: `poc/p2/run_matrix.sh`, `poc/p2/summarize.py`, 시험용 plugin `poc/p2/plugin`(표식 `lucia-p2-marker`가 든 Bash 명령을 PreToolUse 훅이 종료 코드 2로 막는다). 원본 기록은 `poc/raw/p2/`(git 제외).

### 실행 방식

- 데몬은 `sbx exec -i <샌드박스> claude -p …`로 실행한다. 인자, stdin/stdout(stream-json), 종료 코드를 그대로 다룰 수 있다. 샌드박스가 멈춰 있으면 exec가 먼저 시작한다.
- `sbx run --name <샌드박스> -- -p "…"`는 종료 코드 0으로 끝났지만 붙는 메시지와 터미널 제목 제어 문자만 나오고 응답이 없었다. `sbx run`은 대화형으로 붙는 용도이고 킷 진입 명령(`claude --dangerously-skip-permissions`)을 거치므로 쓰지 않는다.

### 권한 모드·훅 조합 (같은 요청: Bash로 `/tmp/lucia-p2-marker`를 touch)

| 경우 | 실행 조합 | 권한 모드(init) | 표식 파일 | 근거 |
|---|---|---|---|---|
| c1 | `--setting-sources project` | default | 안 생김 | 승인 필요로 거부(`permission_denials` 1건) |
| c2 대조군 | c1 + `--dangerously-skip-permissions` | bypassPermissions | **생김** | 같은 요청이면 실행된다 |
| c3 | c2 + `--plugin-dir /opt/lucia/p2/plugin` | bypassPermissions | 안 생김 | PreToolUse 훅 실행, 종료 코드 2로 차단 |
| c4 | 킷 사용자 설정 그대로(플래그 없음) + plugin | bypassPermissions | 안 생김 | 훅이 차단 |
| c5 | `--setting-sources project --permission-prompts none` | default | 안 생김 | 승인할 사람이 없어 거부 |
| c6 | `--restricted --permission-prompts none` + plugin | default | 안 생김 | `--restricted`에서도 `--plugin-dir` 훅이 읽히고 차단 |
| c7 | `--restricted --dangerously-skip-permissions` | — | — | 시작 거부: `bypassPermissions not supported in restricted mode` |

- **bypass 상태에서도 PreToolUse 훅의 거부가 지켜진다**(c3, c4). 문서에는 명시가 없어 실험으로 확인했다.
- bypass는 **데몬이 고를 수 있다**: 킷은 사용자 설정(`permissions.defaultMode`)과 진입 명령으로 bypass를 켜지만, `sbx exec`로 띄우면 둘 다 피할 수 있다(c1, c5, c6).

### 작업 공간을 통한 자기 권한 상승

- 작업 공간(쓰기 가능 마운트)에 `.claude/settings.json`을 심었을 때:
  - `permissions.defaultMode: bypassPermissions`는 무시됐다(2.1.257부터 프로젝트 파일은 bypass를 켤 수 없다고 문서에도 있다).
  - **훅은 실행됐다.** `SessionStart` 훅의 `touch`가 `--tools ""`(Bash 없음) 실행에서도 돌았다. 작업 공간에 쓸 수 있는 실행이 하나라도 있으면 다음 실행에서 임의 명령을 돌릴 수 있다는 뜻이다.
- `--restricted`는 관리형 설정과 `--settings`만 읽으므로 심은 훅이 돌지 않았다(훅 이벤트 0건). `--setting-sources project`에서는 돌았다.

### VM 안 통제와 호스트 통제

- VM 사용자 `agent`는 **비밀번호 없는 sudo**가 있고 docker 그룹이며, `~/.claude`, `~/.claude.json`, `claude` 실행 파일을 소유한다. Bash를 쓸 수 있는 실행은 다음 실행의 설정·훅·실행 파일을 바꿀 수 있다.
- 읽기 전용 마운트는 VM의 root도 못 뚫는다: `mount(2)`로 마운트 표시를 `rw`로 바꾸는 데는 성공했지만 쓰기는 `EROFS`로 실패했고 호스트에 파일이 생기지 않았다(virtiofs 공유가 호스트 쪽에서 읽기 전용).

| 통제 | 어디서 지켜지나 | 유지 범위 |
|---|---|---|
| 네트워크 정책, 프록시의 인증 주입 | 호스트(sbx 데몬) | 모든 실행 |
| 데몬 MCP 도구의 승인 확인(D7) | 호스트(luciad) | 모든 실행 |
| 읽기 전용 마운트(plugin, 설정, 캐릭터) | 호스트(virtiofs) | 모든 실행 |
| PreToolUse 훅, 권한 모드, `--tools`, `--restricted` | VM 안 Claude Code | **그 실행 하나**. 같은 VM에서 코드 실행 도구가 있었던 실행 뒤로는 보장되지 않는다 |

### 실행 조합에 반영할 것

- 데몬의 기본 실행: `sbx exec -i <샌드박스> claude -p --restricted --settings <ro 설정> --plugin-dir <ro plugin> --mcp-config <ro> --strict-mcp-config --tools <명시 목록> --permission-prompts none`(승인 도구를 쓰는 실행은 P6에서 정한다). 환경 변수 `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`(P7에서 확인).
- Bash처럼 코드를 실행하는 도구가 필요한 일(개발 위임 등)은 상주 샌드박스가 아닌 **별도 샌드박스**에서 한다. 상주 샌드박스에서 코드 실행 도구를 한 번이라도 열면 그 뒤 실행의 VM 안 통제를 믿을 수 없다.
- 킷 `CLAUDE.md`(20 KB)는 `--setting-sources project` 실행의 입력 토큰(약 3,400)으로 보아 읽히지 않는다(호스트 실행 3,570과 비슷). `--restricted`도 같다고 보지만 P7에서 다시 본다.
- `-p` 실행 뒤에도 VM 인증 파일에 `trustedDeviceToken`은 생기지 않았다.

## P1 — sbx 설치와 Claude 로그인 (2026-11-16, 통과)

### 결론

- 사용자가 샌드박스 안에서 `/login`(URL을 다른 기기 브라우저에서 열고 코드 붙여넣기)을 하면, 호스트에 전역 시크릿 `anthropic (oauth configured)`가 생긴다. 헤드리스 서버에서 문제없이 됐다.
- 다른 샌드박스는 로그인 없이 `claude -p`가 된다(전역 공유). 로그인한 샌드박스를 지워도(`sbx rm`) 시크릿은 남고, 새로 만든 샌드박스도 그대로 동작한다.
- VM 안 `~/.claude/.credentials.json`의 `accessToken`·`refreshToken`은 26자짜리 자리표시 값(`sk-ant-oat…`, `sk-ant-ort…`)이다. 환경 변수에도 토큰이 없다.
- **예외**: 대화형 `/login`을 한 샌드박스에는 Claude Code가 `trustedDeviceToken`(43자, 실제 값)을 남긴다. 원격 세션·기기 연결 기능에서 `X-Trusted-Device-Token` 헤더로 쓰인다. 바이너리 문자열로 보면 액세스 토큰과 함께 쓰이므로 혼자서는 쓸모가 없어 보이지만(추정, 미확인) 원칙 1의 예외다. **운영 절차**: `/login`은 임시 샌드박스에서 하고 바로 지운다. 이렇게 만든 샌드박스에는 이 토큰이 없다(확인함).
- 만료: 로그인한 샌드박스의 첫 실행에서 `credential_expires_at`이 로그인 시점 + 약 30일(2026-12-15)로 왔다. 다른 샌드박스의 실행에는 이 필드가 없었다. 리프레시 토큰이 갱신되며 연장되는지는 모른다. **감시**: 인증 오류(`is_error`·`terminal_reason=api_error`·"Not logged in")를 바로 알리고, 로그인 날짜를 기록해 25일째에 미리 알린다(명세의 setup-token 11개월 리마인더와 같은 방식).
- 남긴 것: 데몬 전용 시스템 사용자로 돌리는 것, `sbx login`(Docker) 없이 도는지와 Docker 로그인 만료 시 실패 형태 → MVP 배포 단계(sudo와 의도적 로그아웃이 필요하고, 로그아웃하면 실행 중인 샌드박스가 모두 멈춘다).

환경: sbx v0.51.0(v0.53.0 업데이트 있음, 버전 고정 방침에 따라 올리지 않음), VM 안 Claude Code 2.1.325(호스트와 같음), VM 커널 7.0.14, VM 사용자 `agent`.

### 설치와 데몬

- sbx는 사용자별 데몬 `sandboxd`(`sbx daemon start`)를 쓴다. 상태는 `~/.local/state/sandboxes/`. CLI가 필요하면 데몬을 자동으로 띄우며, **데몬은 띄운 프로세스의 그룹을 물려받는다.** kvm 그룹이 없는 셸에서 처음 띄우면 `/dev/kvm: permission denied`로 VM을 만들 수 없다.
  - 이번에는 `sbx daemon stop` 후 `sg kvm -c 'sbx ls'`로 다시 띄웠다(데몬 GID 992=kvm, PPid 1).
  - 운영: 데몬 전용 사용자의 systemd 유닛에 `SupplementaryGroups=kvm`이 필요하다(P3).
- `sbx diagnose`로 설치 상태를 점검할 수 있다(가상화, 인증, 정책, 소켓). 감시(HealthMonitor)에 쓸 수 있다.
- 텔레메트리 끔: `sbx settings set telemetry.disabled true`(데몬 재시작 필요). 진단 자동 업로드: `diagnostics.autoUpload=no`. 기본값 `""`은 오류 때 묻는 상태라 무인 실행을 막을 수 있다.
- 비대화형 실행 시 확인을 묻는 명령은 `--force`나 전역 `--no-input`이 필요하다(예: `sbx policy rm`).
- 기본 샌드박스 메모리가 11.1 GiB로 잡혔다(호스트 22 GB). 상주·브라우저 샌드박스를 함께 띄우려면 크기를 정해야 한다.

### 샌드박스와 Claude 킷

- `sbx create --name lucia-poc claude ~/.local/share/lucia/poc/workspace`: 작업 공간은 VM 안 같은 경로에 쓰기 가능하게 마운트된다. 레포는 마운트하지 않는다.
- 킷이 넣어 두는 것: `~/.claude/settings.json`(`permissions.defaultMode=bypassPermissions`, sbx 상태 보고용 훅), `~/.claude/CLAUDE.md`(20 KB, 샌드박스 사용법), `~/.claude.json`의 사용자 MCP 서버 `mcp-gateway`(sbx MCP 게이트웨이), 호스트 공유 스킬 저장소 마운트 `~/.claude/skills`.
- 킷 문서상 대화형 실행은 `claude --dangerously-skip-permissions`이고, bypass는 **CLI 플래그가 아니라 사용자 설정**으로도 켜진다.
- `sbx exec -i lucia-poc claude -p --input-format stream-json ... --setting-sources project --strict-mcp-config`로 stream-json 왕복이 된다. 이때 init의 `permissionMode`는 `default`였다. 사용자 설정(bypass)을 건너뛰었다는 뜻이다 → P2에서 도구 권한으로 확인.
- 20 KB `CLAUDE.md`가 `--setting-sources project`에서도 읽히는지는 아직 확인하지 않았다(맥락 크기·캐시에 영향) → P7.

### 인증 정보

- VM 환경 변수의 `GH_TOKEN`은 `gho_sbxp…` 자리표시 값이고, 해시가 호스트의 실제 `gh auth token`과 다르다. `sbx secret ls`는 비어 있다. 실제 토큰은 VM에 없다.
- `SBX_CRED_ANTHROPIC_MODE=none`(로그인 전). 킷 문서상 Anthropic 인증은 프록시가 중개한다: API 키면 `apiKeyHelper`가 `proxy-managed`를 돌려주고, 구독이면 `~/.claude/.credentials.json`에 프록시가 바꿔 끼우는 자리표시 토큰이 들어간다.
- 로그인 전 `claude -p` 결과: `subtype=success`인데 `is_error=true`, `result="Not logged in · Please run /login"`, `terminal_reason=api_error`, 모델 `<synthetic>`, 종료 코드 1. **인증 실패 감지는 `subtype`이 아니라 `is_error`와 `terminal_reason`으로 한다.**

### 네트워크

- 전역 정책 `deny-all`(`sbx policy init deny-all`). 전역 허용 규칙 없음.
- Claude 킷이 샌드박스마다 수정할 수 없는 허용 규칙 8개를 추가한다: `api.anthropic.com`, `platform.claude.com`, `claude.com`, `bridge.claudeusercontent.com`, `mcp-proxy.anthropic.com`, `downloads.claude.ai`(443), `deb.debian.org`, `dhi.io`(80). 명세의 최소 허용보다 넓다. 거부가 허용보다 우선하므로 P4에서 샌드박스별 거부 규칙으로 좁힌다(예: 자동 업데이트, 패키지 저장소).
- Claude Code 2.1.325 바이너리의 OAuth 주소: 토큰 교환 `https://platform.claude.com/v1/oauth/token`, 승인 페이지 `https://claude.com/cai/oauth/authorize`(사용자 브라우저에서 열림).
- 킷 `CLAUDE.md`의 호스트 접근 안내: VM에서 호스트 localhost 서비스는 `host.docker.internal:<포트>`, 정책에서 `localhost:<포트>` 허용 필요 → P4에서 확인.

## 호스트 사전 확인 (2026-11-16)

sbx 없이 **호스트의** Claude Code 2.1.325로 실행 계층 조각을 확인했다. 샌드박스 안 동작은 다를 수 있으므로 P5·P6 통과로 세지 않는다. 같은 확인을 P2 이후 샌드박스 안에서 다시 한다.

- 명령: `luciad poc --workdir <빈 디렉터리> --model haiku --record-dir <기록 디렉터리>`
- 실행 조합(`lucia-runner::ClaudeCommand`): `-p --input-format stream-json --output-format stream-json --verbose`, 첫 요청 `--session-id <uuid>`, 두 번째 `--resume <uuid>`, `--mcp-config <파일> --strict-mcp-config`, `--tools ""`, `--allowedTools mcp__lucia__report_result`, `--setting-sources project`. 자식 환경은 비우고 `HOME PATH LANG USER LOGNAME`만 넘긴다.
- MCP 설정: `{"mcpServers":{"lucia":{"type":"http","url":"http://127.0.0.1:<port>/mcp"}}}`. 데몬은 rmcp 3.10 `StreamableHttpService`.

결과:

| 항목 | 결과 |
|---|---|
| 같은 세션 이어 붙이기 | 두 요청의 `session_id`가 지정한 UUID와 같다 |
| MCP 연결 | init 이벤트에 `{"name":"lucia","source":"dynamic","status":"connected"}` |
| 구조화된 결과 보고 | 두 요청 모두 `mcp__lucia__report_result`를 한 번씩 호출. 권한 모드 `default`에서 `--allowedTools`로 허용해 프롬프트·거부 없음 |
| 맥락 유지 | 첫 요청에서 준 단어를 두 번째 보고의 summary에 정확히 적었다 |
| 대조군 | 같은 작업 디렉터리에서 새 세션으로 같은 질문 → `needs_input / 모름`. 작업 디렉터리 기준 auto-memory 디렉터리도 비어 있었다. 맥락 유지는 세션 재개 덕분이다 |
| 보고 짝짓기 | 요청마다 들어온 보고를 모두 꺼내 정확히 한 건인지 확인했다 |
| 비용 | haiku로 6회(파서용 1회, 대조군 없는 첫 실행 2회, 대조군 넣은 실행 3회), API 환산 약 $0.006. 구독 사용량에서 차감 |

stream-json에서 새로 안 것(계획에 반영할 것):

- `result` 이벤트에 `total_cost_usd`가 구독 인증에서도 온다 → 비용 원장에 그대로 쓴다.
- `result.credential_expires_at`(ISO 8601)이 온다 → Claude 인증 만료 감지(명세 원칙 8)에 쓸 수 있다.
- `result.modelUsage.<모델>.contextWindow`가 온다(haiku 5.5는 1,000,000).
- `result.usage`는 **턴 합계**다(턴마다 같은 맥락을 다시 센다). 현재 컨텍스트 크기는 `usage.iterations`의 마지막 항목으로 계산한다(`RunResult::context_tokens`). 예: 2턴 실행에서 합계 9,275, 마지막 턴 4,740.
- Claude Code가 작업 디렉터리마다 auto-memory 디렉터리(`~/.claude/projects/<경로>/memory/`)를 만든다. 데몬의 기억 저장소 밖에 있는 두 번째 저장소라 변경 기록과 인젝션 스캔을 거치지 않는다 → P7에서 공식 문서로 끄는 방법을 찾는다.
- `rate_limit_event.rate_limit_info`에 `five_hour`·`seven_day` 창별 사용률과 리셋 시각이 온다 → 사용량 한도 대응(명세 §스케줄링)에 쓴다.
- `system/thinking_tokens` 같은 문서에 없는 이벤트가 섞인다 → 파서는 모르는 이벤트를 원본으로 남긴다.
- `--input-format stream-json`의 요청 한 줄 형식: `{"type":"user","message":{"role":"user","content":"..."}}`.

픽스처: `crates/lucia-runner/tests/fixtures/host-2.1.325-*.jsonl`(경로와 포트를 가렸다). `tools/fake-claude`가 이 파일을 재생해 `ClaudeCommand::run`을 모델 없이 테스트한다.

## 사전 조사 (2026-11-16, 공식 문서)

출처: <https://docs.docker.com/ai/sandboxes/> (install, get-started, faq, security/defaults, governance/local). 문서에 적힌 내용일 뿐 아직 이 호스트에서 확인하지 않았다.

- 설치: Ubuntu 24.04 이상. 이 호스트는 Docker apt 저장소가 이미 등록되어 있고 `docker-sbx` 0.51.0(ubuntu 26.04 빌드)이 설치 후보다. 로컬 샌드박스는 KVM과 `kvm` 그룹 소속이 필요하다.
- `sbx login`은 Docker 계정 OAuth이고 브라우저를 연다고 되어 있다. 헤드리스에서 어떻게 되는지는 문서에 없다 → P1.
- 로그인이 필요한 이유(FAQ "Why do I need to sign in?"): 샌드박스를 실제 사람과 묶는 신원(거버넌스), 조직·감사 로그 같은 팀 기능, Docker 인프라 인증("Sandboxes pull images, run daemons, and talk to Docker services"). CLI와 로컬 실행은 무료다. 로그인 없이 로컬 샌드박스가 도는지, 오프라인에서 되는지, 로그인을 얼마나 자주 다시 해야 하는지는 문서에 없다 → P1.
- 텔레메트리: 실행한 명령, 성공 여부, 소요 시간, 로그인했다면 Docker 사용자 이름을 보낸다. 프롬프트와 코드는 보내지 않는다고 되어 있다. 끄기: `sbx settings set telemetry.disabled true` 후 `sbx daemon restart`, 또는 `SBX_NO_TELEMETRY=1`. 진단 업로드 도메인도 있으나 무엇을 언제 올리는지는 문서에 없다.
- Claude 구독: 샌드박스 안에서 `/login`으로 OAuth 로그인하며, "세션 토큰은 호스트에 남고 샌드박스 안에 저장되지 않는다"고 되어 있다. 샌드박스 사이 공유 여부는 문서에 없다 → P1.
- 자격 증명 주입: 호스트 측 프록시가 나가는 HTTP 헤더에 넣고, 에이전트는 원래 값을 읽을 수 없다.
- 헤드리스 리눅스의 `sbx secret`은 키링이 없으면 `~/.config/com.docker.sandboxes` 아래 파일(0700)에 저장된다. 파일을 읽을 수 있는 사용자나 프로세스는 값을 꺼낼 수 있다(명세 §인증과 비밀 관리의 주의와 일치).
- 권한 프롬프트: 문서는 "샌드박스 자체가 안전 경계"라고 하고, 프롬프트를 다시 켜려면 실행 명령을 환경 파일이나 Dockerfile에서 바꾸라고 한다. `--dangerously-skip-permissions`를 기본으로 붙이는지는 명시돼 있지 않다 → P2.
- 에이전트 인자 전달(`sbx run claude -- -p ...`), `sbx exec`, 백그라운드 유지, 재부팅 후 상태: 문서에 없다(`sbx run` 레퍼런스 페이지는 본문이 비어 있었다) → P2, P3. `sbx --help`로 확인한다.
- 네트워크: 기본은 나가는 TCP 전부 차단. 프리셋은 `allow-all`, `balanced`, `deny-all`. `sbx policy init balanced`, `sbx policy ls`, `sbx policy allow network <host[:port]>`. deny 규칙이 allow보다 우선한다. VM에서 호스트 서비스로 접근하는 방법은 읽은 페이지에 없다 → P4.
- 샌드박스는 에이전트가 끝나도 남는다. `sbx ls`, `sbx stop`, `sbx rm`. 설치한 패키지와 설정은 재시작 뒤에도 유지된다.
