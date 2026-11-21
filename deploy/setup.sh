#!/usr/bin/env bash
# Lucia 처음 올리기: 묻고 답하며 준비물 확인 → sbx·Claude 로그인 → 설정 파일 → 샌드박스 → 설치 → 관리 페이지 링크까지 간다.
# docs/DEPLOY.md의 순서를 그대로 따른다. 이미 된 단계는 건너뛰므로 중간에 멈췄다가 다시 돌려도 된다.
#
#   deploy/setup.sh            # 차례로 묻고 진행한다
#   deploy/setup.sh --check    # 아무것도 바꾸지 않고 지금 상태만 보여 준다
#
# 받아 쓰는 설치(결정 0065: 이 스크립트의 레포 자리에 빌드해 둔 luciad와 VERSION이 있다)에서는 빌드하지 않는다.
# 그때는 Rust와 Node.js가 필요 없다.
#
# sudo가 필요한 준비물(kvm 그룹에 넣기, sbx 설치, 빠진 작은 도구)은 Ubuntu 24.04 이상에서만, 실행할 명령을 그대로
# 보여 주고 물은 뒤에 한다(기본은 하지 않음). 다른 배포판에서는 명령만 알려 준다.
# 토큰·키 같은 비밀은 묻지 않는다(그것은 설치 뒤 관리 페이지의 "초기 설정"에서 넣는다).
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="${LUCIA_SETUP_CONFIG:-$HOME/.config/lucia/lucia.toml}"
BIN="$HOME/.local/lib/lucia/luciad"
NODE_BIN="${LUCIA_NODE_BIN:-$HOME/.local/opt/node/bin}"
SANDBOX="${LUCIA_SANDBOX:-lucia}"
SOCKET="$HOME/.local/state/sandboxes/sandboxes/sandboxd/sandboxd.sock"
MIN_RUST="1.93"
# 받아 쓰는 설치인가(빌드해 둔 바이너리가 이 스크립트의 레포 자리에 있다).
PREBUILT=0
[[ -x $REPO/luciad && -f $REPO/VERSION ]] && PREBUILT=1
CHECK_ONLY=0
RESTART_NEEDED=0
TOTAL=8

# ---------- 보이기 ----------

if [[ -t 1 ]]; then
  B=$'\e[1m' DIM=$'\e[2m' G=$'\e[32m' Y=$'\e[33m' R=$'\e[31m' C=$'\e[36m' N=$'\e[0m'
else
  B="" DIM="" G="" Y="" R="" C="" N=""
fi
step() { printf '\n%s[%s/%s] %s%s\n' "$B" "$1" "$TOTAL" "$2" "$N"; }
ok() { printf '  %s✓%s %s\n' "$G" "$N" "$*"; }
todo() { printf '  %s·%s %s\n' "$Y" "$N" "$*"; }
bad() { printf '  %s✗%s %s\n' "$R" "$N" "$*"; }
note() { printf '    %s%s%s\n' "$DIM" "$*" "$N"; }
cmd() { printf '    %s$ %s%s\n' "$C" "$*" "$N"; }
die() {
  printf '\n%s멈춥니다:%s %s\n' "$R" "$N" "$*" >&2
  exit 1
}

# 예/아니오. $2가 기본값(y 또는 n). --check에서는 묻지 않고 "아니오"다.
ask() {
  local answer hint="[y/N]"
  [[ ${2:-n} == y ]] && hint="[Y/n]"
  [[ $CHECK_ONLY == 1 ]] && return 1
  read -r -p "  $1 $hint " answer || die "입력이 끊겼습니다."
  answer=${answer:-${2:-n}}
  [[ $answer =~ ^[Yy] ]]
}

# 값 하나. $2가 기본값.
ask_value() {
  local answer
  read -r -p "  $1 [$2] " answer || die "입력이 끊겼습니다."
  printf '%s' "${answer:-$2}"
}

# ---------- 도우미 ----------

have() { command -v "$1" >/dev/null 2>&1; }

# 버전 $1이 $2 이상인가(점으로 나눈 숫자).
version_ge() {
  [[ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1)" == "$2" ]]
}

# 설정 파일의 값 하나를 바꾼다: toml_set <구역 또는 ""> <키> <TOML 값 그대로>.
# 그 구역 안의 주석이 아닌 `키 =` 줄을 바꾸고, 없으면 구역 머리 바로 아래에 넣는다(구역이 없으면 끝에 만든다).
toml_set() {
  python3 -I - "$CONFIG" "$1" "$2" "$3" <<'PY'
import re, sys
path, section, key, value = sys.argv[1:5]
lines = open(path, encoding="utf-8").read().split("\n")
start, end = 0, len(lines)
if section:
    start = next((i + 1 for i, l in enumerate(lines) if l.strip() == f"[{section}]"), None)
    if start is None:
        lines += ["", f"[{section}]", f"{key} = {value}"]
        open(path, "w", encoding="utf-8").write("\n".join(lines).rstrip("\n") + "\n")
        sys.exit(0)
    end = next((i for i in range(start, len(lines)) if re.match(r"\s*\[", lines[i])), len(lines))
else:
    end = next((i for i, l in enumerate(lines) if re.match(r"\s*\[", l)), len(lines))
pattern = re.compile(rf"\s*{re.escape(key)}\s*=")
for i in range(start, end):
    if pattern.match(lines[i]):
        lines[i] = f"{key} = {value}"
        break
else:
    lines.insert(start, f"{key} = {value}")
open(path, "w", encoding="utf-8").write("\n".join(lines))
PY
}

# 설정 파일의 값 하나를 읽는다(없으면 빈 글): toml_get <구역 또는 ""> <키>
toml_get() {
  python3 -I - "$CONFIG" "$1" "$2" <<'PY'
import re, sys
path, section, key = sys.argv[1:4]
try:
    lines = open(path, encoding="utf-8").read().split("\n")
except OSError:
    sys.exit(0)
inside = not section
for line in lines:
    s = line.strip()
    if s.startswith("["):
        inside = s == f"[{section}]"
        continue
    m = re.match(rf"\s*{re.escape(key)}\s*=\s*(.*)", line)
    if inside and m:
        print(m.group(1).strip())
        break
PY
}

# 이 서버의 사설 LAN 주소 후보(Docker·가상 인터페이스는 뺀다).
lan_addresses() {
  have ip || return 0
  ip -4 -o addr show scope global 2>/dev/null |
    awk '$2 !~ /^(docker|br-|veth|virbr|tun|tap|wg|tailscale|cni|flannel)/ {split($4, a, "/"); print a[1]}' |
    grep -E '^(10\.|192\.168\.|172\.(1[6-9]|2[0-9]|3[01])\.)' || true
}

sbx_daemon_running() { [[ -S $SOCKET ]] && pgrep -x sbx >/dev/null 2>&1; }

# 도는 sbx 데몬이 kvm 그룹으로 떠 있나.
sbx_daemon_has_kvm() {
  local gid pid
  gid=$(getent group kvm | cut -d: -f3)
  [[ -n $gid ]] || return 1
  for pid in $(pgrep -x sbx); do
    grep -Eq "^Gid:.*\b$gid\b|^Groups:.*\b$gid\b" "/proc/$pid/status" 2>/dev/null && return 0
  done
  return 1
}

# sbx를 부른다. 데몬이 없고 이 셸에 kvm 그룹이 없으면 sg kvm으로 불러, 처음 뜨는 데몬이 kvm 그룹을 물려받게 한다.
sbx_() {
  # 상태만 볼 때는 데몬을 새로 띄우지 않는다(sbx CLI는 데몬이 없으면 스스로 띄운다).
  if [[ $CHECK_ONLY == 1 ]] && ! sbx_daemon_running; then
    return 1
  fi
  if sbx_daemon_running || [[ " $(id -nG) " == *" kvm "* ]]; then
    sbx "$@"
  else
    sg kvm -c "sbx $(printf '%q ' "$@")"
  fi
}

diag_line() { sbx_ diagnose 2>&1 | grep -F "$1" | head -n1 | sed 's/^ *//'; }

# ---------- 단계 ----------

# 준비물을 본다. 빠진 것 가운데 sudo로 채울 수 있는 것은 NEED_*에 적어 둔다. 빠진 것이 있으면 1.
NEED_KVM_GROUP=0
NEED_SBX=0
NEED_APT=()
check_requirements() {
  local missing=0 v tool
  NEED_KVM_GROUP=0
  NEED_SBX=0
  NEED_APT=()

  if [[ -e /dev/kvm ]]; then ok "KVM 가상화(/dev/kvm)"; else
    bad "/dev/kvm이 없습니다. 가상화가 되는 리눅스 서버가 필요합니다(BIOS의 가상화 설정, VM이면 중첩 가상화)."
    missing=1
  fi

  if id -nG "$USER" 2>/dev/null | tr ' ' '\n' | grep -qx kvm; then ok "kvm 그룹"; else
    bad "사용자 $USER 이(가) kvm 그룹에 없습니다."
    NEED_KVM_GROUP=1
    missing=1
  fi

  if have sbx; then ok "sbx $(sbx version 2>/dev/null | awk '{print $3; exit}')"; else
    bad "sbx(Docker Sandboxes)가 없습니다: https://docs.docker.com/ai/sandboxes/install/"
    NEED_SBX=1
    missing=1
  fi

  if [[ $PREBUILT == 1 ]]; then
    ok "빌드해 둔 바이너리 $(cat "$REPO/VERSION") (Rust와 Node.js는 필요 없습니다)"
  else
    if have cargo && have rustc; then
      v=$(rustc --version 2>/dev/null | awk '{print $2}')
      if [[ -z $v ]]; then
        bad "rustc가 실행되지 않습니다(기본 툴체인이 없을 수 있습니다)."
        cmd "rustup default stable"
        missing=1
      elif version_ge "$v" "$MIN_RUST"; then ok "Rust $v"; else
        bad "Rust $v: $MIN_RUST 이상이 필요합니다."
        cmd "rustup update stable"
        missing=1
      fi
    else
      bad "Rust(cargo)가 없습니다: https://www.rust-lang.org/tools/install"
      note "빌드하지 않고 받아 쓰려면: curl -fsSL https://github.com/mooner1022/lucia/releases/latest/download/install.sh | bash"
      missing=1
    fi

    if have node; then ok "Node.js $(node --version)"; elif [[ -x $NODE_BIN/node ]]; then
      ok "Node.js $("$NODE_BIN/node" --version) ($NODE_BIN)"
    else
      bad "Node.js가 없습니다(관리 화면 빌드에만 씁니다). PATH에 두거나 $NODE_BIN 에 공식 배포판을 풉니다."
      note "https://nodejs.org/dist/  ·  다른 곳에 있으면 LUCIA_NODE_BIN=<node가 있는 디렉터리>"
      missing=1
    fi
  fi

  for tool in python3 systemd-creds systemctl; do
    if have "$tool"; then ok "$tool"; else
      bad "$tool 이(가) 없습니다."
      [[ $tool == python3 ]] && NEED_APT+=(python3)
      missing=1
    fi
  done
  if have systemctl && ! systemctl --user show-environment >/dev/null 2>&1; then
    bad "systemd 사용자 세션에 닿지 못합니다(SSH로 로그인한 그 사용자의 셸에서 돌리세요)."
    missing=1
  fi
  return $missing
}

# sudo로 준비물을 채워도 되는 곳인가: Ubuntu 24.04 이상(Docker가 sbx를 지원하는 곳), apt와 sudo가 있다.
can_install() {
  have sudo && have apt-get && [[ -r /etc/os-release ]] || return 1
  local id version
  id=$(. /etc/os-release && echo "${ID:-}")
  version=$(. /etc/os-release && echo "${VERSION_ID:-0}")
  [[ $id == ubuntu ]] && version_ge "$version" "24.04"
}

# 명령 하나를 보여 주고, 물은 뒤에 돌린다. 기본은 하지 않는다.
run_with_consent() {
  local about=$1
  shift
  echo "  $about"
  cmd "$*"
  ask "실행할까요?" n || return 1
  bash -c "$*"
}

# 빠진 준비물 가운데 sudo로 채울 수 있는 것을 묻고 채운다(명령은 Docker 문서의 것 그대로다).
offer_install() {
  [[ $NEED_KVM_GROUP == 1 || $NEED_SBX == 1 || ${#NEED_APT[@]} -gt 0 ]] || return 1
  if ! can_install; then
    note "이 배포판에서는 대신 설치하지 않습니다(Ubuntu 24.04 이상에서만 합니다). 아래를 직접 실행한 뒤 다시 돌려 주세요."
    [[ ${#NEED_APT[@]} -gt 0 ]] && cmd "sudo apt install ${NEED_APT[*]}"
    [[ $NEED_KVM_GROUP == 1 ]] && cmd "sudo usermod -aG kvm $USER"
    if [[ $NEED_SBX == 1 ]]; then
      cmd "curl -fsSL https://get.docker.com | sudo REPO_ONLY=1 sh"
      cmd "sudo apt install docker-sbx"
    fi
    return 1
  fi
  echo
  echo "  ${B}빠진 준비물을 채웁니다${N}(sudo 비밀번호를 물을 수 있습니다). 하나씩 묻습니다."
  local did=0
  if [[ ${#NEED_APT[@]} -gt 0 ]]; then
    run_with_consent "빠진 도구를 설치합니다." "sudo apt install ${NEED_APT[*]}" && did=1
  fi
  if [[ $NEED_KVM_GROUP == 1 ]]; then
    run_with_consent "VM을 만들 수 있게 $USER 을(를) kvm 그룹에 넣습니다." "sudo usermod -aG kvm $USER" && did=1
  fi
  if [[ $NEED_SBX == 1 ]]; then
    have curl || run_with_consent "sbx를 받는 데 curl이 필요합니다." "sudo apt install curl" || true
    if run_with_consent "Docker의 apt 저장소를 더합니다(Docker의 설치 스크립트를 받아 root로 돌립니다)." \
      "curl -fsSL https://get.docker.com | sudo REPO_ONLY=1 sh"; then
      run_with_consent "sbx를 설치합니다." "sudo apt install docker-sbx" && did=1
    fi
  fi
  [[ $did == 1 ]]
}

preflight() {
  step 1 "준비물"
  [[ $EUID -eq 0 ]] && die "root로 돌리지 않습니다. 데몬을 돌릴 사용자로 실행하세요."
  check_requirements && return 0
  if [[ $CHECK_ONLY == 1 ]]; then
    [[ $NEED_KVM_GROUP == 1 || $NEED_SBX == 1 ]] && note "진행하면(--check 없이) Ubuntu 24.04 이상에서는 kvm 그룹과 sbx 설치를 물어보고 대신 합니다."
    return 1
  fi
  if offer_install; then
    echo
    echo "  다시 확인합니다."
    check_requirements && return 0
  fi
  die "위의 ✗ 항목을 갖춘 뒤 다시 돌려 주세요."
}

sbx_ready() {
  step 2 "sbx: 데몬, Docker 로그인, 네트워크 정책"
  if sbx_daemon_running && ! sbx_daemon_has_kvm; then
    bad "sbx 데몬이 kvm 그룹 없이 떠 있습니다(VM을 만들지 못합니다)."
    cmd "sbx daemon stop && sg kvm -c 'sbx ls'"
    note "데몬을 멈추면 돌고 있는 샌드박스도 멈춥니다. 직접 실행한 뒤 다시 돌려 주세요."
    [[ $CHECK_ONLY == 1 ]] && return 1
    die "sbx 데몬을 kvm 그룹으로 다시 띄워야 합니다."
  fi
  if [[ $CHECK_ONLY == 1 ]] && ! sbx_daemon_running; then
    todo "sbx 데몬이 아직 없습니다(진행하면 kvm 그룹으로 띄웁니다)."
    return 1
  fi
  sbx_ ls -q >/dev/null 2>&1 || die "sbx를 부르지 못했습니다: sg kvm -c 'sbx ls' 를 직접 돌려 오류를 확인하세요."
  ok "sbx 데몬(kvm 그룹)"

  local line
  line=$(diag_line "Authentication")
  if [[ $line == ✓* ]]; then ok "Docker 로그인"; else
    todo "Docker 로그인이 필요합니다."
    [[ $CHECK_ONLY == 1 ]] && return 1
    note "Docker 계정으로 로그인합니다. 안내에 따라 브라우저에서 승인하세요."
    sbx_ login || die "sbx login이 끝나지 않았습니다."
    line=$(diag_line "Authentication")
    [[ $line == ✓* ]] || die "로그인이 확인되지 않습니다: $line"
    ok "Docker 로그인"
  fi

  line=$(diag_line "Global network policy")
  if [[ $line == ✓* ]]; then ok "전역 네트워크 정책"; else
    todo "전역 네트워크 정책이 아직 없습니다."
    [[ $CHECK_ONLY == 1 ]] && return 1
    note "기본은 모두 막고(deny-all), 필요한 곳은 샌드박스마다 엽니다."
    ask "sbx policy init deny-all 을 실행할까요?" y || die "정책을 정한 뒤 다시 돌려 주세요."
    sbx_ policy init deny-all || die "정책을 정하지 못했습니다."
    ok "전역 네트워크 정책(deny-all)"
  fi
}

claude_login() {
  step 3 "Claude 로그인"
  if sbx_ secret ls 2>/dev/null | grep -q 'anthropic.*configured'; then
    ok "Claude 로그인(sbx 시크릿 anthropic)"
    return 0
  fi
  todo "Claude 구독 로그인이 필요합니다."
  [[ $CHECK_ONLY == 1 ]] && return 1
  cat <<EOF
    임시 샌드박스를 열어 Claude Code를 띄웁니다. 그 안에서:
      1) /login 을 입력합니다
      2) 나온 주소를 다른 기기의 브라우저로 열어 승인하고, 코드를 붙여 넣습니다
      3) /exit 로 나옵니다
    로그인은 호스트에 남아 모든 샌드박스가 함께 씁니다. 임시 샌드박스는 나오면 바로 지웁니다.
EOF
  ask "지금 열까요?" y || die "Claude 로그인 뒤 다시 돌려 주세요."
  local tmp name="lucia-login"
  tmp=$(mktemp -d)
  (cd "$tmp" && sbx_ run --name "$name" claude) || true
  # 로그인한 샌드박스에는 기기 토큰이 남으므로 남겨 두지 않는다. 이 스크립트가 만든 이름만 지운다.
  sbx_ rm --force "$name" >/dev/null 2>&1 || true
  rmdir "$tmp" 2>/dev/null || true
  sbx_ secret ls 2>/dev/null | grep -q 'anthropic.*configured' ||
    die "로그인이 확인되지 않습니다(sbx secret ls 에 anthropic이 없습니다). 다시 돌려 /login을 끝까지 해 주세요."
  ok "Claude 로그인"
}

config_file() {
  step 4 "설정 파일"
  if [[ -f $CONFIG ]]; then
    ok "$CONFIG"
    note "시간대 $(toml_get "" timezone) · 관리 페이지 $(toml_get admin bind)"
    return 0
  fi
  todo "$CONFIG 이(가) 없습니다."
  [[ $CHECK_ONLY == 1 ]] && return 1

  local tz detected lan choice i addresses=()
  detected=$(timedatectl show -p Timezone --value 2>/dev/null || true)
  tz=$(ask_value "시간대" "${detected:-Asia/Seoul}")
  while [[ ! -e /usr/share/zoneinfo/$tz ]]; do
    bad "모르는 시간대입니다: $tz (예: Asia/Seoul, Europe/Berlin)"
    tz=$(ask_value "시간대" "${detected:-Asia/Seoul}")
  done

  mapfile -t addresses < <(lan_addresses)
  echo "  관리 페이지를 어디서 열까요? 다른 기기(휴대폰·노트북)에서 열려면 이 서버의 LAN 주소를 고릅니다."
  echo "    0) 이 서버에서만(127.0.0.1)"
  for i in "${!addresses[@]}"; do echo "    $((i + 1))) ${addresses[$i]}"; done
  choice=$(ask_value "번호" "$([[ ${#addresses[@]} -gt 0 ]] && echo 1 || echo 0)")
  lan=""
  if [[ $choice =~ ^[0-9]+$ ]] && ((choice >= 1 && choice <= ${#addresses[@]})); then
    lan=${addresses[$((choice - 1))]}
  fi

  mkdir -p "$(dirname "$CONFIG")"
  cp "$REPO/deploy/lucia.example.toml" "$CONFIG"
  chmod 600 "$CONFIG"
  toml_set "" timezone "\"$tz\""
  if [[ -n $lan ]]; then
    toml_set admin bind "[\"$lan:8780\", \"127.0.0.1:8780\"]"
  fi
  ok "$CONFIG 을(를) 만들었습니다(시간대 $tz, 관리 페이지 ${lan:-127.0.0.1}:8780)."
  note "공인 주소에는 열지 않습니다. 밖에서 쓰려면 docs/DEPLOY.md 7장의 [public]과 리버스 프록시를 봅니다."
}

resident_sandbox() {
  step 5 "상주 샌드박스($SANDBOX)"
  if sbx_ ls -q 2>/dev/null | grep -qx "$SANDBOX"; then
    ok "샌드박스 $SANDBOX"
    return 0
  fi
  todo "샌드박스 $SANDBOX 이(가) 없습니다."
  [[ $CHECK_ONLY == 1 ]] && return 1
  note "Lucia가 사는 VM을 만듭니다(메모리 상한 기본 4 GB, LUCIA_SANDBOX_MEMORY로 바꿉니다)."
  ask "지금 만들까요?" y || die "샌드박스를 만든 뒤 다시 돌려 주세요."
  "$REPO/deploy/create-sandbox.sh" || die "샌드박스를 만들지 못했습니다."
  ok "샌드박스 $SANDBOX"
}

install_daemon() {
  step 6 "빌드와 설치"
  local active=0
  systemctl --user is-active --quiet luciad.service 2>/dev/null && active=1
  if [[ -x $BIN && $active == 1 ]]; then
    ok "luciad가 설치돼 돌고 있습니다."
    [[ $CHECK_ONLY == 1 ]] && return 0
    if [[ $PREBUILT == 1 ]]; then
      ask "받아 둔 바이너리($(cat "$REPO/VERSION"))로 다시 설치할까요?" n || return 0
    else
      ask "지금 코드로 다시 빌드해 업데이트할까요?" n || return 0
    fi
  else
    todo "아직 설치되지 않았습니다."
    [[ $CHECK_ONLY == 1 ]] && return 1
    if [[ $PREBUILT == 1 ]]; then
      note "받아 둔 바이너리를 systemd 사용자 서비스로 올립니다."
      ask "지금 설치할까요?" y || die "설치한 뒤 다시 돌려 주세요: $REPO/deploy/install.sh"
    else
      note "관리 화면과 데몬을 빌드해 systemd 사용자 서비스로 올립니다. 처음에는 10분쯤 걸립니다."
      ask "지금 빌드하고 설치할까요?" y || die "설치한 뒤 다시 돌려 주세요: deploy/install.sh"
    fi
  fi
  "$REPO/deploy/install.sh" || die "설치하지 못했습니다. 위의 오류를 고친 뒤 다시 돌려 주세요."
  RESTART_NEEDED=0
  ok "설치했습니다."
}

boot_start() {
  step 7 "부팅 때 자동으로 띄우기"
  if [[ "$(loginctl show-user "$USER" -p Linger --value 2>/dev/null)" == yes ]]; then
    ok "linger 켜짐(로그인하지 않아도 부팅 때 뜹니다)"
    return 0
  fi
  todo "linger가 꺼져 있습니다(로그인해야만 서비스가 뜹니다)."
  [[ $CHECK_ONLY == 1 ]] && return 1
  if ask "켤까요? (loginctl enable-linger $USER)" y; then
    loginctl enable-linger "$USER" && ok "linger 켬" || bad "켜지 못했습니다. 직접: sudo loginctl enable-linger $USER"
  fi
}

# 선택 기능 하나: 샌드박스를 만들고 설정을 켠다. optional <구역> <이름> <샌드박스 이름> <만드는 스크립트> <설명>
optional() {
  local section=$1 label=$2 name=$3 script=$4 about=$5
  if [[ "$(toml_get "$section" enabled)" == true ]] && sbx_ ls -q 2>/dev/null | grep -qx "$name"; then
    ok "$label: 켜져 있습니다."
    return 0
  fi
  todo "$label: 꺼져 있습니다. $about"
  [[ $CHECK_ONLY == 1 ]] && return 0
  ask "$label 을(를) 켤까요?" n || return 0
  "$REPO/deploy/$script" || {
    bad "$label 샌드박스를 만들지 못했습니다. 나중에: deploy/$script"
    return 0
  }
  toml_set "$section" enabled true
  RESTART_NEEDED=1
  ok "$label: 켰습니다."
}

finish() {
  step 8 "선택 기능과 마무리"
  if [[ -f $CONFIG ]]; then
    # 설정에 샌드박스 이름이 없으면 데몬의 기본 이름이다.
    local browser_box script_box
    browser_box=$(toml_get browser sandbox | tr -d '"')
    script_box=$(toml_get scripts sandbox | tr -d '"')
    optional browser "브라우저" "${browser_box:-lucia-browser}" create-browser-sandbox.sh \
      "사이트 로그인과 페이지 조작(메모리 2 GB)."
    optional scripts "코드 실행" "${script_box:-lucia-script}" create-script-sandbox.sh \
      "Lucia가 쓴 Python 코드와 반복 루틴(메모리 1 GB)."
  fi
  [[ $CHECK_ONLY == 1 ]] && return 0

  if [[ $RESTART_NEEDED == 1 ]]; then
    note "설정이 바뀌어 데몬을 다시 시작합니다."
    systemctl --user restart luciad.service || bad "다시 시작하지 못했습니다: journalctl --user -u luciad -n 50"
    sleep 3
  fi

  echo
  echo "  ${B}점검${N}(모델을 부르지 않습니다)"
  "$BIN" check 2>&1 | sed 's/^/  /' || true

  echo
  echo "  ${B}관리 페이지${N}"
  local out
  if out=$("$BIN" admin-setup 2>&1); then
    printf '%s\n' "$out" | sed 's/^/  /'
    cat <<EOF

  위 링크(30분 유효)에서 비밀번호를 정하고 로그인하면 "초기 설정" 화면이 뜹니다.
  거기서 이름과 프로필 그림, Discord 봇, Google 메일·일정, 앱 푸시를 넣습니다.
EOF
  else
    note "비밀번호가 이미 있습니다. 관리 페이지에 로그인해 메뉴 → 관리 → 초기 설정으로 갑니다."
    note "관리 페이지: http://$(toml_get admin bind | grep -oE '[0-9.]+:[0-9]+' | head -n1)"
    note "비밀번호를 잊었으면: $BIN admin-setup --reset"
  fi
  cat <<EOF

  ${B}끝났습니다.${N}
    로그      journalctl --user -u luciad -f
    업데이트  $([[ $PREBUILT == 1 ]] && echo "curl -fsSL https://github.com/mooner1022/lucia/releases/latest/download/install.sh | bash" || echo "git pull && deploy/install.sh")
    걷어 내기  $REPO/deploy/uninstall.sh
    자세히    $([[ $PREBUILT == 1 ]] && echo "$REPO/docs/DEPLOY.md" || echo "docs/DEPLOY.md")
EOF
}

main() {
  case "${1:-}" in
    --check) CHECK_ONLY=1 ;;
    -h | --help)
      sed -n '2,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    "") ;;
    *) die "모르는 인자: $1 (--check, --help)" ;;
  esac

  printf '%sLucia 처음 올리기%s  %s\n' "$B" "$N" "$REPO"
  if [[ $CHECK_ONLY == 1 ]]; then
    echo "지금 상태만 봅니다. 아무것도 바꾸지 않습니다."
    local pending=0
    preflight || pending=1
    if have sbx; then
      sbx_ready || pending=1
      claude_login || pending=1
    fi
    config_file || pending=1
    have sbx && { resident_sandbox || pending=1; }
    install_daemon || pending=1
    boot_start || pending=1
    finish
    echo
    if [[ $pending == 1 ]]; then
      echo "남은 단계가 있습니다. 진행하려면: deploy/setup.sh"
      exit 1
    fi
    echo "모두 되어 있습니다."
    exit 0
  fi

  # LUCIA_SETUP_ALLOW_PIPE=1은 시험용이다(답을 표준 입력으로 넣는다).
  [[ -t 0 || ${LUCIA_SETUP_ALLOW_PIPE:-0} == 1 ]] ||
    die "묻고 답하는 스크립트입니다. 터미널에서 돌려 주세요(상태만 보려면 --check)."
  echo "이미 된 단계는 건너뜁니다. 중간에 멈추려면 Ctrl+C, 다시 돌리면 이어서 합니다."
  preflight
  sbx_ready
  claude_login
  config_file
  resident_sandbox
  install_daemon
  boot_start
  finish
}

# 시험에서 함수만 가져다 쓸 수 있게, 직접 돌릴 때만 시작한다.
if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
  main "$@"
fi
