# 배포 가이드

Lucia를 자기 리눅스 서버에 처음 올리는 순서다. 끝까지 하면 데몬이 systemd 사용자 서비스로 돌고, 관리 페이지의 "초기 설정"에서 나머지(이름, Discord, Google, 앱 푸시)를 정할 수 있다.

기본은 빌드해 둔 릴리스를 받아서 설치하는 것이다. 터미널에서 한 줄이면 된다.

```sh
curl -fsSL https://github.com/mooner1022/lucia/releases/latest/download/install.sh | bash
```

묶음을 받아 풀고, 처음이면 묻고 답하며 끝까지 데려가는 `deploy/setup.sh`로 이어진다. Rust와 Node.js는 필요 없다(1장). 코드를 고치며 개발하거나 x86-64가 아닌 서버에서는 레포를 받아 직접 빌드한다(1장 "레포에서 빌드하기").

- 레포에서 빌드하면 준비물이 있을 때 20~30분이 걸린다(첫 빌드가 그 가운데 10분쯤). 받아서 설치하면 빌드가 없다.
- 이 문서의 명령은 모두 **데몬을 돌릴 그 사용자**로 실행한다. root로 하지 않는다.
- 받아서 설치했다면 이 문서의 `deploy/…` 명령은 `~/.local/lib/lucia/release`에서 돌린다(`cd ~/.local/lib/lucia/release`). 레포에서 빌드했다면 받은 레포의 위치에서 돌린다.
- 이 순서는 운영 중인 서버 한 대의 구성을 옮겨 적은 것이다. 빈 서버에서 처음부터 끝까지 다시 해 본 것은 아니다. 받아서 설치하는 길은 스크립트를 시험용 환경에서만 돌려 보았다. 막히는 곳은 `luciad check`의 출력과 맨 아래 "막혔을 때"를 본다.

## 0. 준비물

두 방식이 모두 필요로 하는 것이다.

| 무엇 | 왜 | 확인 |
|---|---|---|
| 리눅스 서버(x86-64, systemd, KVM 가상화) | 에이전트가 sbx의 가상 머신 안에서 돈다 | `ls /dev/kvm` |
| [Docker Sandboxes(`sbx`)](https://docs.docker.com/ai/sandboxes/)와 Docker 계정 | 가상 머신. Docker의 제품이라 Lucia의 묶음에 넣을 수 없다 | `sbx version` (v0.51에서 확인했다. 더 새로운 판은 확인하지 못했다) |
| `kvm` 그룹에 든 사용자 | sbx가 VM을 만든다 | `id -nG`에 `kvm` |
| Claude 구독(Pro·Max) | Lucia의 실행 엔진이 Claude Code다 | |
| `python3` | 한 줄 설치(받은 파일 맞추기), `setup.sh`·`uninstall.sh`(설정 파일 읽고 쓰기), 브라우저·코드 실행 샌드박스를 만드는 스크립트, sbx 유닛(`lucia-sbx.service`)의 확인 | `python3 --version` |
| `systemd-creds --user`(**systemd 256 이상**) | 자격 증명 암호화. Ubuntu 24.04(systemd 255)에서는 되지 않는다. 26.04에서 확인했다 | `systemctl --version` |
| 메모리 | 상주 4 GB, 브라우저 2 GB(선택), 스크립트 1 GB(선택)가 기본 상한이다 | |

한 줄 설치 스크립트는 시작할 때 `curl`, `tar`, `sha256sum`, `python3`가 있는지 본다. 없으면 `sudo apt install <도구>`를 알려 주고 멈춘다(이 단계에서는 대신 설치하지 않는다).

레포에서 빌드할 때만 더 필요한 것이다.

| 무엇 | 왜 | 확인 |
|---|---|---|
| `git` | 레포 받기 | `git --version` |
| Rust 1.93 이상 | 데몬 빌드 | `rustc --version` |
| Node.js(최근 LTS 이상. 26에서 확인했다) | 관리 화면 빌드에만 쓴다 | `node --version` |

Node를 시스템에 깔지 않으려면 공식 배포판을 `~/.local/opt/node`에 풀어도 된다(`deploy/build-ui.sh`가 `~/.local/opt/node/bin`을 찾는다. 다른 곳이면 `LUCIA_NODE_BIN`).

### 준비물을 대신 설치해 주는 범위

`deploy/setup.sh`는 빠진 준비물 가운데 sudo가 필요한 것을 **Ubuntu 24.04 이상에서만**(`sudo`와 `apt-get`이 있을 때) 대신 설치해 준다. 실행할 명령을 그대로 보여 주고, 하나씩 묻고(기본은 "아니오"), 승낙한 것만 한다. sudo 비밀번호를 물을 수 있다. `--check`에서는 아무것도 묻지도 하지도 않는다. systemd가 256보다 낮은 것은 대신 채울 수 없다(더 새 배포판이 필요하다).

| 빠진 것 | 하는 명령 |
|---|---|
| `kvm` 그룹 | `sudo usermod -aG kvm $USER` |
| `sbx` | `curl -fsSL https://get.docker.com \| sudo REPO_ONLY=1 sh` 뒤 `sudo apt install docker-sbx` |
| `python3` | `sudo apt install python3` |

- sbx 명령은 [Docker의 설치 문서](https://docs.docker.com/ai/sandboxes/install/)에 있는 그대로다. 첫 줄은 Docker의 apt 저장소를 더하려고 Docker의 설치 스크립트를 받아 root로 돌린다. 그 스크립트를 믿을 수 없으면 승낙하지 않고 문서를 보고 직접 한다. `curl`이 없으면 그 앞에 `sudo apt install curl`을 먼저 묻는다.
- 다른 배포판이나 `sudo`가 없는 곳에서는 같은 명령을 보여 주기만 하고 멈춘다(보여 주는 명령은 apt 기준이다). 직접 한 뒤 다시 돌린다.
- 한 줄 설치로 오면 `python3`는 이 단계 전에 이미 있어야 하므로, 이 제안에 닿는 것은 레포에서 `setup.sh`를 직접 돌릴 때뿐이다.
- 그룹에 막 넣은 셸에는 `kvm`이 아직 적용되지 않는다. `setup.sh`는 그럴 때 `sg kvm`으로 sbx를 부른다.

대화할 곳은 둘 가운데 하나가 있어야 한다. 설치 뒤 관리 페이지에서 정한다.

- **Discord 봇**(가장 빠르다): Discord 개발자 포털에서 만든 봇의 토큰과 내 사용자 ID.
- **앱**: 접속 API(`docs/app-api.md`)로 붙는 앱. 안드로이드 클라이언트는 [Lucent](https://github.com/mooner1022/lucent)다(다른 저장소).

## 1. 설치하기

### 받아서 설치하기(기본)

```sh
curl -fsSL https://github.com/mooner1022/lucia/releases/latest/download/install.sh | bash
```

x86-64 리눅스에서, root가 아닌 사용자로 돌린다. 하는 일:

1. GitHub 릴리스에서 묶음(`lucia-x86_64-linux.tar.gz`)과 체크섬을 받아 맞춰 본다. 맞지 않으면 멈춘다. 체크섬은 받다가 깨지지 않았는지만 본다. 누가 만들었는지는 증명하지 않는다(받는 곳이 이 저장소의 GitHub 릴리스라는 것이 근거다).
2. `~/.local/lib/lucia/release`에 푼다. 바이너리(관리 화면이 들어 있다), `deploy/`의 스크립트와 서비스 유닛, `plugin/`, 기본 캐릭터가 든다. 이 디렉터리는 자리를 지킨 채 내용만 바뀐다(상주 샌드박스가 그 안의 `plugin/`을 붙여 쓴다).
3. 처음이면 `release/deploy/setup.sh`로 넘어가 아래 "한 번에 하기"를 한다. 이미 설치돼 있으면 `release/deploy/install.sh`로 업데이트한다("업데이트와 되돌리기").

- `curl | bash`로 돌려도 `setup.sh`의 질문은 터미널(`/dev/tty`)에서 읽는다. 터미널이 없으면 풀기까지만 하고, 이어서 돌릴 명령(`~/.local/lib/lucia/release/deploy/setup.sh`)을 찍고 끝낸다.
- `LUCIA_VERSION=v0.1.0`: 받을 릴리스를 고른다(기본은 가장 새것). 파이프의 `bash` 쪽에 둔다.

  ```sh
  curl -fsSL https://github.com/mooner1022/lucia/releases/latest/download/install.sh | LUCIA_VERSION=v0.1.0 bash
  ```

- `LUCIA_NO_SETUP=1`: 풀기만 하고 설정·업데이트로 넘어가지 않는다(`… | LUCIA_NO_SETUP=1 bash`).
- `~/.local/lib/lucia/release`는 한 줄 설치가 관리하는 자리다. 옮기지 않고, 안에 파일을 만들지 않는다. 업데이트가 묶음에 없는 파일을 지운다.
- 레포에서 빌드한 서버에서는 이 명령이 내려받기 전에 멈춘다(두 방식을 한 서버에서 섞지 않는다). 그런 서버는 레포에서 업데이트한다: `git pull && deploy/install.sh`.

### 레포에서 빌드하기

개발하거나 x86-64가 아닐 때 쓴다(받는 바이너리는 Ubuntu 24.04의 glibc로 빌드해 그보다 새 배포판에서 돈다). 어느 길이든 systemd 256 이상이 필요하다. Arm에서는 시험하지 못했다.

```sh
git clone https://github.com/mooner1022/lucia.git ~/lucia
cd ~/lucia
```

받은 위치는 **옮기지 않는다.** 상주 샌드박스가 이 디렉터리의 `plugin/`과 `character/`를 읽기 전용으로 붙여 쓰고, 서비스 유닛도 이 위치를 가리킨다. 옮기려면 샌드박스를 다시 만들고 `deploy/install.sh`를 다시 돌린다.

### 한 번에 하기

받아서 설치하면 한 줄 설치가 이 스크립트까지 불러 준다. 레포에서 빌드했다면 직접 돌린다.

```sh
deploy/setup.sh
```

아래 2~6장을 묻고 답하며 차례로 해 주는 스크립트다. 준비물을 확인하고(빠진 것은 위 "준비물을 대신 설치해 주는 범위"), sbx와 Claude 로그인을 안내하고, 설정 파일을 만들고(시간대와 관리 페이지를 열 주소를 묻는다), 샌드박스를 만들고, 설치한 뒤(레포에서 빌드하면 이때 빌드한다) 관리 페이지 비밀번호 링크를 찍는다. 끝에서 브라우저와 코드 실행(7장)을 켤지도 묻는다.

- 이미 된 단계는 건너뛴다. 중간에 멈췄거나 한 단계가 실패했으면 고친 뒤 다시 돌린다.
- `deploy/setup.sh --check`는 아무것도 바꾸지 않고 어디까지 됐는지만 보여 준다.
- 토큰·키 같은 비밀은 묻지 않는다. 그것은 설치 뒤 관리 페이지의 초기 설정에서 넣는다.

아래 장들은 같은 일을 손으로 하는 순서이고, 스크립트가 무엇을 하는지의 설명이기도 하다. 두 방식이 같되, 다른 곳에서는 짚는다.

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
cd -                                  # 아까 있던 곳으로 돌아간다
```

## 3. 설정 파일

```sh
mkdir -p ~/.config/lucia
cp deploy/lucia.example.toml ~/.config/lucia/lucia.toml
```

처음에는 두 가지만 본다.

- `timezone`: 내 시간대.
- `[admin] bind`: 예시는 `127.0.0.1:8780`뿐이다. 다른 기기(휴대폰, 노트북)에서 관리 페이지를 열려면 **이 서버의 LAN 주소**를 더한다. 예: `bind = ["192.168.0.10:8780", "127.0.0.1:8780"]`. 공인 주소에는 바인드하지 않는다.

나머지(`[browser]`, `[scripts]`, `[gateway]`, `[public]`)는 꺼진 채 시작하고 7장에서 켠다. 비밀 값은 이 파일에 쓰지 않는다. 토큰과 키는 관리 페이지가 받아 `~/.config/lucia/credentials/`에 암호화해 둔다.

## 4. 상주 샌드박스 만들기

```sh
deploy/create-sandbox.sh
```

Lucia가 사는 VM(`lucia`)을 만들고, 데몬 포트만 열고, 열렸는지와 막혔는지를 출력한다. 이미 있으면 아무것도 하지 않는다. VM에는 `plugin/`(훅)과 캐릭터가 읽기 전용으로 붙는다.

- 레포에서 빌드: 레포의 `plugin/`과 `character/`가 붙는다.
- 받아서 설치: 풀어 둔 릴리스의 `plugin/`이 붙고, 캐릭터는 `~/.config/lucia/character`가 붙는다. 그 안에 `character.md`가 없을 때만 기본 캐릭터를 복사해 넣으므로, 관리 페이지에서 고친 캐릭터가 업데이트로 덮이지 않는다.
- 샌드박스는 한 번 만들면 마운트가 고정된다. 업데이트는 마운트가 아니라 그 안의 내용을 바꾼다.

## 5. 설치하고 띄우기

```sh
deploy/install.sh
loginctl enable-linger "$USER"     # 로그인하지 않아도 부팅 때 뜨게(한 번)
```

`install.sh`가 하는 일:

1. 바이너리를 마련한다. 레포에서 빌드하면 관리 화면 빌드와 시험 → 릴리스 빌드. 받아서 설치하면 풀어 둔 바이너리를 쓴다(빌드하지 않는다).
2. 새 바이너리에 화면이 온전히 들어 있는지 본다. 아니면 설치하지 않는다.
3. 하는 중인 작업이 있으면 멈춘다(`--force`로 건너뛴다).
4. (있으면) 저장소를 백업한다.
5. `~/.local/lib/lucia/luciad`를 설치하고, systemd 사용자 유닛 `luciad.service`·`lucia-sbx.service`를 설치해 시작한다.

업데이트할 때도 같은 명령이다. 레포에서 빌드했다면 `git pull` 뒤 `deploy/install.sh`, 받아서 설치했다면 한 줄 설치를 다시 돌린다("업데이트와 되돌리기").

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

Lucia의 이름·말투·호칭은 관리 페이지 "캐릭터"에서 고친다. 고치는 파일은 레포에서 빌드했다면 레포의 `character/character.md`, 받아서 설치했다면 `~/.config/lucia/character/character.md`다.

## 7. 선택 기능 켜기

필요한 것만 한다. 각각 샌드박스를 만들고, 설정을 켜고, 데몬을 다시 시작한다(`systemctl --user restart luciad`). 받아서 설치했다면 `deploy/…` 명령은 `~/.local/lib/lucia/release`에서 돌린다(관리 페이지와 `luciad check`가 알려 주는 `deploy/…` 명령도 마찬가지다).

| 기능 | 만들기 | 설정(`~/.config/lucia/lucia.toml`) | 확인 |
|---|---|---|---|
| 브라우저(사이트 로그인, 페이지 조작) | `deploy/create-browser-sandbox.sh` | `[browser] enabled = true` | `luciad check` |
| 코드 실행(Python, 반복 루틴) | `deploy/create-script-sandbox.sh` | `[scripts] enabled = true` | `luciad script-check` |
| 자체 앱 접속 API | | `[gateway] bind = ["<LAN 주소>:8790"]` | 관리 페이지 → 연결 |
| 밖에서 접속 | 리버스 프록시(TLS)를 앞에 둔다. 예: `deploy/traefik/docker-compose.yml`(값은 `deploy/traefik/env.example`을 `.env`로 복사해 적는다) | `[public]` | 밖에서 로그인하려면 집 안에서 2단계 인증을 먼저 켠다 |

- 브라우저 샌드박스에는 로그인된 쿠키가, 상주 샌드박스에는 대화 기록이 들어 있다. 만든 뒤에는 `sbx rm`하지 않는다.
- 접속 API와 관리 페이지는 집 LAN이나 VPN 주소에만 바인드한다. 밖으로는 `[public]`과 프록시로만 연다.
- 받아서 설치했다면 값은 `~/.local/lib/lucia/release/deploy/traefik/.env`에 둔다. 업데이트는 사용자가 만든 파일을 지우지 않는다(묶음에 든 `docker-compose.yml`을 고친 것은 덮어쓴다).

## 업데이트와 되돌리기

### 받아서 설치한 경우

처음 설치할 때와 같은 한 줄이다.

```sh
curl -fsSL https://github.com/mooner1022/lucia/releases/latest/download/install.sh | bash
```

- 설치돼 있으면 `setup.sh` 대신 `release/deploy/install.sh`로 간다. 묶음을 받아 `release/`를 맞춘 뒤 위 5장의 순서(화면 확인, 하는 중인 작업 확인, 저장소 백업, 바이너리 설치, 재시작)를 한다.
- 새 바이너리는 뜰 때 저장소를 마이그레이션한다. 그래서 먼저 `~/.local/share/lucia/backup/lucia.sqlite.<시각>`에 사본을 만든다(최근 20개). 사본을 만들지 못하면 설치하지 않는다.
- 하는 중인 작업이 있거나 저장소의 상태를 읽지 못하면 멈춘다. 그렇게 멈춰도 묶음은 이미 `release/`에 풀려 있다(`plugin/`은 새 것이고, 돌고 있는 데몬은 옛 바이너리다). 작업이 끝난 뒤 `~/.local/lib/lucia/release/deploy/install.sh`만 다시 부르면 된다(다시 받지 않는다). 기다리지 않으려면 `~/.local/lib/lucia/release/deploy/install.sh --force`(하는 중인 작업은 실패로 닫힌다). `--force`와 `--no-restart` 같은 인자는 한 줄 설치로는 넘길 수 없다.
- 하던 답이 끝나기를 잠깐(기본 180초, `LUCIA_DEPLOY_WAIT`) 기다렸다가 재시작한다.
- 특정 판으로 맞추려면 `… | LUCIA_VERSION=<태그> bash`(옛 판으로 내리는 것은 시험하지 않았다).
- 되돌리기: `mv ~/.local/lib/lucia/luciad.prev ~/.local/lib/lucia/luciad && systemctl --user restart luciad`. 되돌리는 것은 바이너리뿐이다. `release/`의 `plugin/`과 스크립트는 새 것으로 남는다. 저장소 모양이 바뀐 배포였다면 백업한 저장소도 함께 되돌린다.

### 레포에서 빌드한 경우

```sh
cd ~/lucia && git pull && deploy/install.sh
```

- 새 바이너리는 뜰 때 저장소를 마이그레이션한다. 그래서 `install.sh`가 먼저 `~/.local/share/lucia/backup/`에 사본을 만든다(최근 20개).
- 하는 중인 작업이 있으면 멈추고, 하던 답이 끝나기를 잠깐 기다렸다가 재시작한다.
- 되돌리기: `mv ~/.local/lib/lucia/luciad.prev ~/.local/lib/lucia/luciad && systemctl --user restart luciad`. 저장소 모양이 바뀐 배포였다면 백업한 저장소도 함께 되돌린다.

## 걷어 내기

```sh
deploy/uninstall.sh --check    # 무엇이 남아 있는지만 본다(아무것도 지우지 않는다)
deploy/uninstall.sh            # 묶음마다 묻고 지운다
```

받아서 설치했다면 `~/.local/lib/lucia/release/deploy/uninstall.sh`다. `deploy/setup.sh`와 `deploy/install.sh`가 이 서버에 만든 것을 차례로 걷어 낸다: 서비스 → 샌드박스 → sbx 데몬 유닛 → 바이너리 → 설정과 자격 증명 → 데이터 → 그 밖(밖에서 접속하던 라벨 컨테이너, 레포 안의 빌드 결과, linger).

- 묶음마다 묻고, 답 없이 Enter를 치면 지우지 않는다. 남겨 둔 것은 끝에 모아 보여 준다.
- **되돌릴 수 없는 것**은 샌드박스(상주 대화의 기록, 로그인된 브라우저 프로필)와 데이터(기억, 파일, 백업)다. 데이터는 지우기 전에 한 파일로 묶어 둘지 묻고, 지울 때는 낱말을 직접 치게 한다.
- 이름이 `lucia`, `lucia-…`이거나 설정에 적힌 샌드박스만 지운다. 다른 샌드박스는 건드리지 않는다.
- Claude 로그인, 전역 네트워크 정책, Docker 로그인은 다른 sbx 샌드박스도 함께 쓰므로 따로 묻는다(기본은 그대로 둔다).
- 지우지 않는 것: 레포에서 빌드했다면 그 레포 디렉터리, sbx·Rust·Node 같은 도구. 홈 밖에 둔 데이터 디렉터리는 지우지 않고 명령만 알려 준다.
- 받아서 설치했다면: `~/.local/lib/lucia/release`는 "바이너리" 단계에서 `~/.local/lib/lucia`와 함께 지워진다. "설정과 자격 증명" 단계는 설정 디렉터리 전체를 지우므로 `~/.config/lucia/character`(관리 페이지에서 고친 캐릭터)도 함께 지워진다. 끝 안내의 "레포"(`rm -rf` 안내) 줄은 레포에서 빌드한 설치를 기준으로 한 것이라, 받아서 설치했다면 그 줄의 `release`는 이미 지워졌을 수 있다.
- **바깥 서비스의 토큰은 여기서 지워도 살아 있다.** 더 쓰지 않을 것이면 Discord 개발자 포털(봇 토큰), Google 계정의 앱 접근 권한, Firebase 서비스 계정 키, OpenAI API 키를 그쪽에서 거둔다.

## 어디에 무엇이 있나

| 경로 | 내용 |
|---|---|
| `~/.config/lucia/lucia.toml` | 설정(비밀 없음) |
| `~/.config/lucia/credentials/` | 암호화한 토큰·키(`systemd-creds`). 이 서버, 이 사용자만 푼다 |
| `~/.local/share/lucia/` | 저장소(`lucia.sqlite`), 작업 공간, 파일, 백업(`backup/`) |
| `~/.local/lib/lucia/luciad` | 운영 바이너리(직전 것은 `luciad.prev`) |
| `~/.local/lib/lucia/release/` | (받아서 설치) 풀어 둔 묶음: `luciad`, `VERSION`, `deploy/`, `plugin/`, `character/`(기본 캐릭터), `README.md`, `docs/DEPLOY.md`. 서비스 유닛의 작업 디렉터리이고 상주 샌드박스가 `plugin/`을 붙인다. 업데이트가 내용을 맞추므로 안에 만든 파일은 지워진다 |
| `~/.config/lucia/character/character.md` | (받아서 설치) 쓰는 캐릭터. 관리 페이지 "캐릭터"가 고치고, 업데이트가 덮지 않는다 |
| `<레포>/character/character.md` | (레포에서 빌드) 쓰는 캐릭터 |
| `~/.config/systemd/user/luciad.service`, `lucia-sbx.service` | 서비스 유닛 |

서버를 옮길 때는 `~/.local/share/lucia/`와 설정 파일(받아서 설치했다면 `~/.config/lucia/character/`도)을 옮기고, 자격 증명은 새 서버의 관리 페이지에서 다시 넣는다(암호화 키가 서버마다 다르다).

## 막혔을 때

먼저 `deploy/setup.sh --check`와 `luciad check`로 어디가 안 됐는지 본다.

| 증상 | 볼 것 |
|---|---|
| 한 줄 설치가 "내려받지 못했습니다" | 릴리스가 있는지 <https://github.com/mooner1022/lucia/releases>. `LUCIA_VERSION`을 줬다면 그 태그가 있는지 |
| 한 줄 설치가 "체크섬이 맞지 않습니다" | 받다가 깨진 것이다. 다시 돌린다 |
| 한 줄 설치가 도구가 필요하다며 멈춘다 | `sudo apt install <도구>`(`curl`, `tar`, `sha256sum`, `python3`) |
| 업데이트가 "하는 중인 작업이 N개 있어 멈춥니다" | 끝난 뒤 `~/.local/lib/lucia/release/deploy/install.sh`(받아서 설치한 경우) 또는 `deploy/install.sh`. `--force`는 하는 중인 작업을 실패로 닫는다 |
| 업데이트가 "저장소의 상태를 읽지 못해 멈춥니다" | 저장소 파일(`~/.local/share/lucia/lucia.sqlite`)이 있는지, 데이터 디렉터리가 기본이 아니면 `LUCIA_DATA_DIR`. 확인한 뒤 다시 하거나 `--force` |
| `luciad check`나 관리 화면이 알려 준 `deploy/…` 명령이 없다고 나온다 | 받아서 설치했다면 `cd ~/.local/lib/lucia/release` 뒤에 돌린다 |
| `luciad check`의 "sbx 데몬 kvm 권한"이 ✗ | `sbx daemon stop` 뒤 `sg kvm -c 'sbx ls'` |
| "Claude 로그인(sbx 시크릿)"이 ✗ | 2장의 Claude 로그인을 다시 한다 |
| "상주 샌드박스"가 ✗ | `deploy/create-sandbox.sh` |
| 서비스가 바로 죽는다 | `journalctl --user -u luciad -n 50`. 대개 설정 파일의 주소·포트다(데몬이 까닭을 한 줄로 적는다) |
| 관리 페이지가 안 열린다 | `[admin] bind`에 그 기기에서 닿는 주소가 있는지. 서버에서만 열린다면 `ssh -L 8780:127.0.0.1:8780 <서버>`로 잠깐 볼 수 있다 |
| `admin-setup`이 "이미 비밀번호가 있다" | 잊었다면 `luciad admin-setup --reset` |
| (레포에서 빌드) 화면 빌드에서 `node`를 못 찾는다 | PATH에 두거나 `LUCIA_NODE_BIN=<node가 있는 디렉터리>` |
| 재부팅 뒤 안 뜬다 | `loginctl enable-linger "$USER"` |

## 알아 둘 것

- **한 사람용이다.** 소유자 한 명의 메일·일정·계정을 다루도록 만들었고, 여러 사용자를 나누는 기능은 없다.
- **모델 사용량이 든다.** 대화, 아침 브리핑, 후속 조치가 Claude 구독 사용량을 쓴다. 상한은 관리 페이지 설정에 있다.
- **Lucia가 바깥에 남기는 일**(메일 발송, 폼 제출, 게시)은 승인 카드로 묻는다. 권한 모드와 승인 규칙은 `docs/STATUS.md`의 권한 장에 있다.
- 다른 서비스와 같이 쓰는 서버라면 샌드박스의 메모리 상한(`LUCIA_SANDBOX_MEMORY` 등, 각 스크립트의 머리말)을 맞춘다.
- 받아서 설치하는 묶음은 x86-64용 하나뿐이다. 설치 스크립트들은 sbx v0.51로 확인했고, 처음 설치하는 사람은 그보다 새로운 sbx를 받을 수 있다.
