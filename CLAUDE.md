# Lucia

홈서버에서 상시 구동하는 1인용 개인 비서 에이전트. Claude Code를 실행 엔진으로 쓰고, 그 바깥(기억, 스케줄, 상태, 알림, 승인)을 Rust 데몬 `luciad`가 맡는다.

## 먼저 읽을 문서

- `docs/spec.pdf`: 작업 명세(기준 문서). 이 호스트에는 poppler가 없어 Read 도구로 PDF를 바로 열 수 없으므로 `docs/spec.txt`(뽑아 둔 텍스트, 표 두 개는 맨 위에 다시 정리)를 읽는다. 시스템 구성도(2쪽)는 그림이라 텍스트에 없다.
- `docs/PLAN.md`: 개발 계획. 단계, PoC 실험, 미결정 사항 대응표.
- `docs/decisions/`: 확정된 결정. 계획과 다르게 가려면 여기에 기록을 남긴다.
- `poc/RESULTS.md`: PoC 실험 결과와 sbx 사전 조사.

## 레포 구조

- `crates/luciad`: 데몬 바이너리(액터형, tokio mpsc, SQLite 하나)
- `crates/lucia-core`: 플랫폼 중립 타입(메시지 모델, 작업 상태 머신, 권한 등급)
- `crates/lucia-runner`: 실행 백엔드(`claude -p`, stream-json, sbx)
- `crates/lucia-channel`: 채널 어댑터(Discord)와 공통 계약 테스트
- `crates/lucia-precheck`: 모델 없이 변화를 감지하는 사전 체크 모듈
- `tools/fake-claude`: 기록된 stream-json 재생기(테스트용)
- `plugin/`: 에이전트에게 주는 Claude Code plugin(스킬, 훅, MCP 설정)
- `character/character.md`: 캐릭터 본체(`--append-system-prompt-file`)
- `poc/`, `deploy/`, `migrations/`

## 명령

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all

# 운영 데몬은 systemd 사용자 서비스다(결정 0012): luciad.service(바이너리 ~/.local/lib/lucia/luciad)와
# lucia-sbx.service(sbx 데몬). 배포 = 릴리스 빌드 → 운영 저장소 백업 → 설치 → 재시작:
deploy/install.sh
journalctl --user -u luciad -f        # 로그
systemctl --user restart luciad       # 재시작(VM은 멈추지 않는다)
# 데몬을 직접 띄우기(관리 페이지 http://<LAN 주소>:8780, Discord, 스케줄러). 운영 서비스가 MCP 포트(47810)와
# 브라우저 프록시 포트(47811)를 쥐므로, 운영 설정으로는 돌리지 않는다. 개발용 ask·briefing도 포트를 바꾼 개발 설정으로.
cargo run -p luciad -- run --config <개발용 설정>
# 관리 페이지 초기 비밀번호 링크(터미널에만 찍힌다. 사용자가 직접 실행한다)
cargo run -p luciad -- admin-setup
# 접속 API(자체 앱, 결정 0021·0043, 설명 docs/app-api.md): 설정 [gateway] bind(기본 꺼짐, 집 LAN·VPN 주소만). 토큰은 관리 페이지 "연결"에서.
# 외부 노출(결정 0043): 설정 [public](공개 주소·믿는 프록시, 기본 비어 있음). Traefik은 라벨 컨테이너로 잇는다: docker compose -f deploy/traefik/docker-compose.yml up -d.
# 데몬은 Traefik이나 운영 [public]을 스스로 바꾸지 않는다. 밖의 관리 페이지 로그인에는 집 안에서 켠 2단계 인증이 필요하다.
# 운영 점검(모델 호출 없음). 설정: ~/.config/lucia/lucia.toml(예시: deploy/lucia.example.toml)
cargo run -p luciad -- check
# Discord 서버 점검(결정 0024): 저장된 봇 토큰으로 REST만 읽는다(게이트웨이에 붙지 않아 운영 데몬과 함께 돌려도 된다).
# 봇이 들어간 서버·관리자 권한·메시지 내용 intent가 켜졌는지. 메시지 내용은 출력하지 않는다.
cargo run -p luciad -- discord-check
# 스크립트 샌드박스(결정 0025): 만들기(없을 때만, 네트워크 허용 없음) → 설정 [scripts] enabled = true → 점검.
# 점검은 모델·데몬 없이 실제 샌드박스에서 코드·도구 다리, 네트워크 막힘, 시간 초과 끝내기를 본다(운영과 함께 돌려도 된다).
# 개발 샌드박스: LUCIA_SCRIPT_SANDBOX=lucia-script-dev deploy/create-script-sandbox.sh (개발 설정의 [scripts] sandbox)
deploy/create-script-sandbox.sh
cargo run -p luciad -- script-check [--config <설정>]
# 상주 세션 기록을 지금 VM 밖으로 백업(데몬은 매일 04:00 루틴으로 한다)
cargo run -p luciad -- backup
# 상주 샌드박스 만들기(없을 때만)
deploy/create-sandbox.sh
# 브라우저 샌드박스 만들기(없을 때만, 결정 0006·0008). 만든 뒤 설정 [browser] enabled = true.
# 네트워크는 데몬 프록시 포트(기본 47811) 하나만 허용한다. 규칙만 다시 걸 때: --policy-only
deploy/create-browser-sandbox.sh
# 개발용: 브라우저 점검(프록시를 띄우고 내장 사이트 첫 페이지 한 번 + VM 안 시험 페이지로 로그인·조작·차단).
# 실제 계정으로는 로그인하지 않는다. 프록시 포트를 쓰므로 데몬과 같은 설정으로 동시에 돌리지 않는다.
# 개발 샌드박스: LUCIA_BROWSER_SANDBOX=<이름> LUCIA_BROWSER_PROXY_PORT=<포트> deploy/create-browser-sandbox.sh 로
# 만들고, 설정의 [browser] sandbox·proxy_bind를 그것에 맞춘다.
cargo run -p luciad -- browser-check [--config <설정>] [--skip-site]
# 좌표·iframe 부분만(결정 0023): LUCIA_BROWSER_CHECK_ONLY=frames cargo run -p luciad -- browser-check --config <개발 설정> --skip-site
# 개발용: 실제 샌드박스로 채팅 한 번·브리핑 한 번(모델 사용). 운영 서비스가 돌고 있어도 되게 개발용 상주
# 샌드박스(lucia-dev, MCP 포트 47820)와 개발용 설정(~/.config/lucia/lucia-dev.toml: 저장소·포트·샌드박스를
# 따로, 브라우저 끔)을 쓴다. 채팅이 작업을 맡기면(task_start) 그 작업도 이어 돌린다(결정 0014).
# 개발 샌드박스 만들기: LUCIA_SANDBOX=lucia-dev LUCIA_WORKSPACE=~/.local/share/lucia-dev/workspace \
#   LUCIA_MCP_PORT=47820 deploy/create-sandbox.sh
# 개발용 브라우저 샌드박스(lucia-browser-dev, 프록시 포트 47821, 개발 설정의 [browser]가 가리킨다):
#   LUCIA_BROWSER_SANDBOX=lucia-browser-dev LUCIA_BROWSER_PROXY_PORT=47821 deploy/create-browser-sandbox.sh
# 개발 샌드박스는 다 쓰면 멈춘다(호스트 메모리를 다른 서비스와 나눈다): sbx stop lucia-dev lucia-browser-dev lucia-script-dev
cargo run -p luciad -- ask --config <개발용 설정> [--file <파일>]... "말"
# stdout 원본을 픽스처용으로 남기려면 LUCIA_RECORD_DIR=<디렉터리>(개발용만, 홈 경로·포트는 가린 뒤 넣는다)
cargo run -p luciad -- briefing --config <개발용 설정>
# 개발용: 내장 루틴 하나를 지금(daily_wrapup, periodic_check, retro, task:<작업 ID>, push:<작업 ID>, dev_cleanup,
# routine:<루틴 ID> — 반복 루틴 하나를 스크립트부터, 깨우면 Lucia까지, 결정 0025)
cargo run -p luciad -- routine --config <개발용 설정> daily_wrapup

# 실행 계층 확인: 같은 세션에 요청 두 번 + report_result 수신. 실제 모델을 부르므로 구독 사용량이 든다.
# 작업 디렉터리는 레포 밖의 빈 디렉터리를 쓴다.
cargo run -p luciad -- poc --workdir <빈 디렉터리> --model haiku [--record-dir <기록 디렉터리>]
```

테스트는 모델을 부르지 않는다. 실행 계층은 `tools/fake-claude`가 `crates/lucia-runner/tests/fixtures/`의 기록을 재생해 검증한다. 새 기록을 픽스처로 넣을 때는 홈 경로, 작업 디렉터리, 포트를 가린다.

## sbx(Docker Sandboxes) 다룰 때

- sbx는 사용자별 데몬(`sbx daemon start`)을 쓰고, **처음 호출한 sbx CLI가 데몬을 자동으로 띄우며 그 프로세스의 그룹을 물려받는다.** kvm 그룹이 없는 셸(예: 로그인 전에 시작한 Claude Code 세션)에서 처음 부르면 VM을 못 만든다. 그때는 `sbx daemon stop` 후 `sg kvm -c 'sbx ls'`로 다시 띄운다. 확인: `pgrep -x sbx`의 `/proc/<PID>/status` Gid가 kvm(992)인지.
- 비대화형으로 부를 때 확인을 묻는 명령은 `--force`나 `--no-input`이 필요하다.
- **호스트를 재부팅하지 않는다.** 이 서버에는 다른 서비스도 돌고 있어, P3(재부팅 복구) 시험은 건너뛰었다. 재부팅이 필요한 작업은 제안하지 말고, 사용자가 유지보수로 재부팅할 때 `poc/p3/after_reboot.sh`를 돌린다.
- PoC 샌드박스 `lucia-poc`의 세션을 그 스크립트가 쓴다(세션 ID와 단어는 레포에 올리지 않는 `poc/p3/.env`에 있다). 샌드박스를 지우면 그 파일의 값을 새로 만든 것으로 바꾼다.
- 개발 작업(결정 0020)은 작업마다 임시 샌드박스 `lucia-task-<작업>`을 만들고, 작업이 끝나면 데몬이 지운다. 이 이름의 샌드박스만 데몬이 지운다.
- 개발용 샌드박스는 `lucia-dev`(상주, MCP 47820), `lucia-browser-dev`(브라우저, 프록시 47821), `lucia-script-dev`(스크립트)다. 운영 설정·포트와 겹치지 않는다. 다 쓰면 `sbx stop`으로 멈춘다.
- 스크립트 샌드박스는 `lucia-script`(`deploy/create-script-sandbox.sh`, 결정 0025). 평소에는 네트워크 허용 규칙을 두지 않는다(코드가 바깥에 닿는 길은 데몬 다리뿐이다). Lucia가 `lib_install`로 Python 패키지를 설치하는 동안만 데몬이 `pypi.org`·`files.pythonhosted.org`를 이 샌드박스에 열었다 닫는다(결정 0047). 설치한 패키지 말고는 안에 남길 것이 없어 다시 만들어도 된다(패키지는 Lucia가 다시 설치한다). sbx는 막힌 곳의 TCP 연결도 받아 두고 끊으므로, 막혔는지는 응답으로 본다(`luciad script-check`).
- 브라우저 샌드박스는 `lucia-browser`(`deploy/create-browser-sandbox.sh`). 로그인된 프로필(쿠키)이 들어 있으므로 함부로 `sbx rm`하지 않는다. 네트워크 규칙은 반드시 `--sandbox lucia-browser`로 건다(빼면 모든 샌드박스에 적용된다).
- 브라우저 출구는 데몬 SOCKS5 프록시 하나다(결정 0008). **sbx 정책에 `**`나 사이트 허용을 더하지 않는다**: sbx는 이름으로 풀린 사설 IP를 막지 못한다(PoC P8b). 갈 곳은 관리 페이지의 차단 목록으로 정한다. 상주 샌드박스 `lucia`에 프록시 포트를 허용하지 않는다(에이전트가 인터넷을 얻는다).
- 상주 샌드박스는 `lucia`(`deploy/create-sandbox.sh`). 상주 세션 기록이 그 안에 있으므로 함부로 `sbx rm`하지 않는다. 코드 실행 도구(Bash 등)를 여는 시험은 상주 샌드박스가 아닌 임시 샌드박스에서 한다(PoC P2: VM 안 통제는 그 실행에만 보장된다).
- 실험 결과와 sbx 동작은 `poc/RESULTS.md`에 있다.

## 지켜야 할 것

- **보호 영역**: `plugin/hooks/`, plugin의 권한 설정, `character/`는 운영 중인 에이전트와 학습 루프가 고칠 수 없는 영역이다. 개발 중에도 사용자가 요청할 때만 바꾼다.
  - 사용자는 캐릭터 파일을 관리 페이지 "캐릭터" 화면에서 고친다(결정 0009). 그 화면은 레포의 `character/character.md`를 직접 고치고 변경 기록을 저장소에 남긴다. 그래서 git 작업 트리에 변경이 보일 수 있다. 사용자 변경이므로 되돌리지 않는다.
- **`character/character.md`는 파일 전체가 매 실행마다 모델에 들어간다**(`--append-system-prompt-file`). 관리용 메모나 주석을 넣지 않는다. 바뀌는 맥락(기억, 진행 상태)도 넣지 않는다. 그건 데몬이 요청 메시지 앞부분에 넣는다. v0는 중립 말투이고, 이름·말투·관계는 사용자가 정해 채운다(D5).
  - 이어 쓰는 상주 대화는 시스템 프롬프트를 압축될 때까지 기록해 둔 그대로 쓴다(`--system-prompt-snapshot` 기본 on). 그래서 캐릭터 변경은 다음 압축이나 새 세션부터 적용된다.
- **인증 정보는 호스트만 가진다**: 토큰, 비밀번호, API 키를 코드, 테스트 픽스처, 로그, PoC 기록에 넣지 않는다. 기록 전에 마스킹한다.
- **`claude --bare`를 쓰지 않는다**: 훅을 건너뛰고 구독 인증(OAuth)을 읽지 않는다.
- **sbx 명령은 공식 문서(<https://docs.docker.com/ai/sandboxes/>)나 `sbx --help`로 확인한 것만 쓴다**. 기억에 의존해 적지 않는다.
- **관리 화면(`crates/luciad/src/admin/view/`)을 고칠 때는 있던 기능을 없애지 않는다**(결정 0041). `view/parity.rs`가 폼·입력·링크·보이던 데이터를 기준 목록(`parity_baseline.txt`)과 맞추고, `view/a11y.rs`가 접근성(라벨, 제목 단계, 대비)을 본다. CSP는 사용자 지시로 해제했다(결정 0042). 지금 화면은 스크립트·인라인 스타일이 없고, 주입 방어는 이스케이프 하나이므로 바깥 글은 반드시 이스케이프한다. 기준 목록은 의도한 제거일 때만 다시 만들고 결정 문서에 까닭을 적는다.
- **명세에 없는 설계**는 PLAN.md에서 [제안]으로 표시하고, 확정되면 `docs/decisions/`에 남긴다.
- 문서와 코드 주석은 한국어로 쓴다. 식별자는 영어.
