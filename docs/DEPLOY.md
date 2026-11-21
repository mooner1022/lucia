# 배포 가이드

Lucia를 자기 리눅스 서버에 처음 올리는 순서다. 끝까지 하면 데몬이 systemd 사용자 서비스로 돌고, 관리 페이지의 "초기 설정"에서 나머지(이름, Discord, Google, 앱 푸시)를 정할 수 있다.

- 걸리는 시간: 준비물이 있으면 20~30분(첫 빌드가 그 가운데 10분쯤).
- 이 문서의 명령은 모두 **데몬을 돌릴 그 사용자**로 실행한다. root로 하지 않는다.
- 이 순서는 운영 중인 서버 한 대의 구성을 옮겨 적은 것이다. 빈 서버에서 처음부터 끝까지 다시 해 본 것은 아니므로, 막히는 곳은 `luciad check`의 출력과 맨 아래 "막혔을 때"를 본다.

## 0. 준비물

| 무엇 | 왜 | 확인 |
|---|---|---|
| 리눅스 서버(x86-64, systemd, KVM 가상화) | 에이전트가 sbx의 가상 머신 안에서 돈다 | `ls /dev/kvm` |
| [Docker Sandboxes(`sbx`)](https://docs.docker.com/ai/sandboxes/)와 Docker 계정 | 가상 머신 | `sbx version` (v0.51에서 확인했다) |
| `kvm` 그룹에 든 사용자 | sbx가 VM을 만든다 | `id -nG`에 `kvm` |
| Claude 구독(Pro·Max) | Lucia의 실행 엔진이 Claude Code다 | |
| Rust 1.93 이상 | 데몬 빌드 | `rustc --version` |
| Node.js(최근 LTS 이상. 26에서 확인했다) | 관리 화면 빌드에만 쓴다 | `node --version` |
| `sqlite3`, `python3`, `systemd-creds`(systemd 250 이상) | 배포 스크립트의 백업, 유닛의 확인, 자격 증명 암호화 | `sqlite3 --version`, `systemd-creds --version` |
| 메모리 | 상주 4 GB, 브라우저 2 GB(선택), 스크립트 1 GB(선택)가 기본 상한이다 | |

Node를 시스템에 깔지 않으려면 공식 배포판을 `~/.local/opt/node`에 풀어도 된다(`deploy/build-ui.sh`가 `~/.local/opt/node/bin`을 찾는다. 다른 곳이면 `LUCIA_NODE_BIN`).

대화할 곳은 둘 가운데 하나가 있어야 한다. 설치 뒤 관리 페이지에서 정한다.

- **Discord 봇**(가장 빠르다): Discord 개발자 포털에서 만든 봇의 토큰과 내 사용자 ID.
- **자체 앱**: 접속 API(`docs/app-api.md`)로 붙는 앱. 앱 자체는 이 저장소에 없다.

## 1. 저장소 받기

```sh
git clone https://github.com/mooner1022/lucia.git ~/lucia
cd ~/lucia
```

받은 위치는 **옮기지 않는다.** 상주 샌드박스가 이 디렉터리의 `plugin/`과 `character/`를 읽기 전용으로 붙여 쓰고, 서비스 유닛도 이 위치를 가리킨다. 옮기려면 샌드박스를 다시 만들고 `deploy/install.sh`를 다시 돌린다.

## 2. sbx 준비(한 번)

```sh
# kvm 그룹이 없는 셸이면 먼저(그룹에 막 넣었다면 다시 로그인한다)
sg kvm -c 'sbx ls'

sbx login                    # Docker 계정 로그인(안내에 따라 브라우저에서 승인한다)
sbx policy init deny-all     # 전역 네트워크 정책: 기본은 모두 막는다
```

- sbx 데몬은 **처음 부른 sbx 명령의 그룹을 물려받는다.** kvm 그룹 없이 처음 불렀다면 `sbx daemon stop` 뒤 `sg kvm -c 'sbx ls'`로 다시 띄운다.
- 전역 정책은 `deny-all`로 둔다. 필요한 곳은 아래 스크립트가 샌드박스마다 연다.

### Claude 로그인

Claude 구독 로그인은 샌드박스 안에서 한 번 하면 호스트의 sbx 시크릿으로 남아 모든 샌드박스가 함께 쓴다. **임시 샌드박스에서 하고 바로 지운다**(로그인한 샌드박스에는 기기 토큰이 남는다).

```sh
mkdir -p /tmp/lucia-login && cd /tmp/lucia-login
sbx run --name lucia-login claude     # 안에서 /login → 나온 주소를 다른 기기 브라우저로 열고 코드를 붙여 넣는다 → /exit
sbx rm --force lucia-login
sbx secret ls                         # anthropic 이 보이면 됐다
cd ~/lucia
```

## 3. 설정 파일

```sh
mkdir -p ~/.config/lucia
cp deploy/lucia.example.toml ~/.config/lucia/lucia.toml
```

처음에는 두 가지만 본다.

- `timezone`: 내 시간대.
- `[admin] bind`: 예시는 `127.0.0.1:8780`뿐이다. 다른 기기(휴대폰, 노트북)에서 관리 페이지를 열려면 **이 서버의 LAN 주소**를 더한다. 예: `bind = ["192.168.0.10:8780", "127.0.0.1:8780"]`. 공인 주소에는 바인드하지 않는다.

나머지(`[browser]`, `[scripts]`, `[gateway]`, `[public]`)는 꺼진 채 시작하고 6장에서 켠다. 비밀 값은 이 파일에 쓰지 않는다. 토큰과 키는 관리 페이지가 받아 `~/.config/lucia/credentials/`에 암호화해 둔다.

## 4. 상주 샌드박스 만들기

```sh
deploy/create-sandbox.sh
```

Lucia가 사는 VM(`lucia`)을 만들고, 데몬 포트만 열고, 열렸는지와 막혔는지를 출력한다. 이미 있으면 아무것도 하지 않는다.

## 5. 설치하고 띄우기

```sh
deploy/install.sh
loginctl enable-linger "$USER"     # 로그인하지 않아도 부팅 때 뜨게(한 번)
```

`install.sh`가 하는 일: 관리 화면 빌드와 시험 → 릴리스 빌드 → (있으면) 저장소 백업 → `~/.local/lib/lucia/luciad` 설치 → systemd 사용자 유닛 `luciad.service`·`lucia-sbx.service` 설치와 시작. 업데이트할 때도 `git pull` 뒤 같은 명령이다.

확인한다.

```sh
systemctl --user status luciad
~/.local/lib/lucia/luciad check      # 모델을 부르지 않는 점검. 모두 ✓여야 한다
journalctl --user -u luciad -f       # 로그
```

## 6. 관리 페이지에서 초기 설정

```sh
~/.local/lib/lucia/luciad admin-setup
```

터미널에 **30분짜리 일회용 링크**가 찍힌다. 그 링크에서 관리 페이지 비밀번호를 정하고 로그인하면 "초기 설정" 화면이 먼저 뜬다(나중에는 메뉴 → 관리 → 초기 설정).

| 단계 | 넣는 것 | 필수 |
|---|---|---|
| 이름과 모습 | 관리 화면과 앱에 보일 이름, 프로필 그림 | 아니오 |
| Discord | 봇 토큰, 내 Discord 사용자 ID | 대화할 곳 하나는 필수 |
| Google 메일·일정 | Google Cloud의 OAuth 클라이언트(데스크톱) JSON → "Google로 연결" | 아니오 |
| 앱과 푸시 | 앱 토큰 발급, 알림 받을 곳, Firebase 서비스 계정 JSON | 앱을 쓸 때만 |
| 이미지 생성 | OpenAI API 키 | 아니오 |
| 환경 | 샌드박스·설정·점검 상태(보기만) | |

Discord를 넣으면 봇이 확인 DM을 보낸다. 그 DM에 말을 걸면 Lucia가 답한다. 여기까지가 최소 구성이다.

Lucia의 이름·말투·호칭은 관리 페이지 "캐릭터"에서 고친다. 저장소의 `character/character.md`가 그 파일이다.

## 7. 선택 기능 켜기

필요한 것만 한다. 각각 샌드박스를 만들고, 설정을 켜고, 데몬을 다시 시작한다(`systemctl --user restart luciad`).

| 기능 | 만들기 | 설정(`~/.config/lucia/lucia.toml`) | 확인 |
|---|---|---|---|
| 브라우저(사이트 로그인, 페이지 조작) | `deploy/create-browser-sandbox.sh` | `[browser] enabled = true` | `luciad check` |
| 코드 실행(Python, 반복 루틴) | `deploy/create-script-sandbox.sh` | `[scripts] enabled = true` | `luciad script-check` |
| 자체 앱 접속 API | | `[gateway] bind = ["<LAN 주소>:8790"]` | 관리 페이지 → 연결 |
| 밖에서 접속 | 리버스 프록시(TLS)를 앞에 둔다. 예: `deploy/traefik/docker-compose.yml`(값은 `deploy/traefik/env.example`을 `.env`로 복사해 적는다) | `[public]` | 밖에서 로그인하려면 집 안에서 2단계 인증을 먼저 켠다 |

- 브라우저 샌드박스에는 로그인된 쿠키가, 상주 샌드박스에는 대화 기록이 들어 있다. 만든 뒤에는 `sbx rm`하지 않는다.
- 접속 API와 관리 페이지는 집 LAN이나 VPN 주소에만 바인드한다. 밖으로는 `[public]`과 프록시로만 연다.

## 업데이트와 되돌리기

```sh
cd ~/lucia && git pull && deploy/install.sh
```

- 새 바이너리는 뜰 때 저장소를 마이그레이션한다. 그래서 `install.sh`가 먼저 `~/.local/share/lucia/backup/`에 사본을 만든다(최근 20개).
- 하는 중인 작업이 있으면 멈추고, 하던 답이 끝나기를 잠깐 기다렸다가 재시작한다.
- 되돌리기: `mv ~/.local/lib/lucia/luciad.prev ~/.local/lib/lucia/luciad && systemctl --user restart luciad`. 저장소 모양이 바뀐 배포였다면 백업한 저장소도 함께 되돌린다.

## 어디에 무엇이 있나

| 경로 | 내용 |
|---|---|
| `~/.config/lucia/lucia.toml` | 설정(비밀 없음) |
| `~/.config/lucia/credentials/` | 암호화한 토큰·키(`systemd-creds`). 이 서버, 이 사용자만 푼다 |
| `~/.local/share/lucia/` | 저장소(`lucia.sqlite`), 작업 공간, 파일, 백업 |
| `~/.local/lib/lucia/luciad` | 운영 바이너리(직전 것은 `luciad.prev`) |
| `~/.config/systemd/user/luciad.service`, `lucia-sbx.service` | 서비스 유닛 |

서버를 옮길 때는 `~/.local/share/lucia/`와 설정 파일을 옮기고, 자격 증명은 새 서버의 관리 페이지에서 다시 넣는다(암호화 키가 서버마다 다르다).

## 막혔을 때

| 증상 | 볼 것 |
|---|---|
| `luciad check`의 "sbx 데몬 kvm 권한"이 ✗ | `sbx daemon stop` 뒤 `sg kvm -c 'sbx ls'` |
| "Claude 로그인(sbx 시크릿)"이 ✗ | 2장의 Claude 로그인을 다시 한다 |
| "상주 샌드박스"가 ✗ | `deploy/create-sandbox.sh` |
| 서비스가 바로 죽는다 | `journalctl --user -u luciad -n 50`. 대개 설정 파일의 주소·포트다(데몬이 까닭을 한 줄로 적는다) |
| 관리 페이지가 안 열린다 | `[admin] bind`에 그 기기에서 닿는 주소가 있는지. 서버에서만 열린다면 `ssh -L 8780:127.0.0.1:8780 <서버>`로 잠깐 볼 수 있다 |
| `admin-setup`이 "이미 비밀번호가 있다" | 잊었다면 `luciad admin-setup --reset` |
| 화면 빌드에서 `node`를 못 찾는다 | PATH에 두거나 `LUCIA_NODE_BIN=<node가 있는 디렉터리>` |
| 재부팅 뒤 안 뜬다 | `loginctl enable-linger "$USER"` |

## 알아 둘 것

- **한 사람용이다.** 소유자 한 명의 메일·일정·계정을 다루도록 만들었고, 여러 사용자를 나누는 기능은 없다.
- **모델 사용량이 든다.** 대화, 아침 브리핑, 후속 조치가 Claude 구독 사용량을 쓴다. 상한은 관리 페이지 설정에 있다.
- **Lucia가 바깥에 남기는 일**(메일 발송, 폼 제출, 게시)은 승인 카드로 묻는다. 권한 모드와 승인 규칙은 `docs/STATUS.md`의 권한 장에 있다.
- 다른 서비스와 같이 쓰는 서버라면 샌드박스의 메모리 상한(`LUCIA_SANDBOX_MEMORY` 등, 각 스크립트의 머리말)을 맞춘다.
