#!/usr/bin/env bash
# Lucia 걷어 내기: deploy/setup.sh와 deploy/install.sh가 이 서버에 만든 것을 묻고 답하며 지운다.
# 서비스 → 샌드박스 → sbx 공용 상태(선택) → sbx 데몬 유닛 → 바이너리 → 설정과 자격 증명 → 데이터 → 그 밖의 순서다.
#
#   deploy/uninstall.sh            # 묶음마다 묻고 지운다
#   deploy/uninstall.sh --check    # 아무것도 지우지 않고, 무엇이 남아 있는지만 보여 준다
#
# 되돌릴 수 없는 것: 샌드박스(대화 기록, 로그인된 브라우저 프로필)와 데이터(기억, 파일, 백업). 데이터는 지우기 전에
# 묶어 둘지 묻고, 지울 때는 낱말을 직접 치게 한다.
# 하지 않는 것: 이 레포 디렉터리, sbx·Rust·Node 같은 도구, 다른 프로그램의 샌드박스는 건드리지 않는다.
# 바깥 서비스의 토큰(Discord 봇, Google 동의, Firebase 키, OpenAI 키)은 여기서 지워도 그쪽에서 살아 있으므로 따로 거둔다.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="${LUCIA_SETUP_CONFIG:-$HOME/.config/lucia/lucia.toml}"
CONFIG_DIR="$(dirname "$CONFIG")"
BIN_DIR="$HOME/.local/lib/lucia"
UNIT_DIR="$HOME/.config/systemd/user"
SOCKET="$HOME/.local/state/sandboxes/sandboxes/sandboxd/sandboxd.sock"
CHECK_ONLY=0
TOTAL=8
LEFT=()

# ---------- 보이기 ----------

if [[ -t 1 ]]; then
  B=$'\e[1m' DIM=$'\e[2m' G=$'\e[32m' Y=$'\e[33m' R=$'\e[31m' C=$'\e[36m' N=$'\e[0m'
else
  B="" DIM="" G="" Y="" R="" C="" N=""
fi
step() { printf '\n%s[%s/%s] %s%s\n' "$B" "$1" "$TOTAL" "$2" "$N"; }
ok() { printf '  %s✓%s %s\n' "$G" "$N" "$*"; }
found() { printf '  %s·%s %s\n' "$Y" "$N" "$*"; }
bad() { printf '  %s✗%s %s\n' "$R" "$N" "$*"; }
note() { printf '    %s%s%s\n' "$DIM" "$*" "$N"; }
cmd() { printf '    %s$ %s%s\n' "$C" "$*" "$N"; }
die() {
  printf '\n%s멈춥니다:%s %s\n' "$R" "$N" "$*" >&2
  exit 1
}
kept() { LEFT+=("$1"); }

# 예/아니오. 기본은 늘 "아니오"다(지우는 일이라). --check에서는 묻지 않는다.
ask() {
  local answer
  [[ $CHECK_ONLY == 1 ]] && return 1
  read -r -p "  $1 [y/N] " answer || die "입력이 끊겼습니다."
  [[ ${answer:-n} =~ ^[Yy] ]]
}

# 되돌릴 수 없는 일: 낱말을 그대로 쳐야 한다.
ask_word() {
  local answer
  [[ $CHECK_ONLY == 1 ]] && return 1
  read -r -p "  $1 지우려면 \"$2\"라고 치세요: " answer || die "입력이 끊겼습니다."
  [[ $answer == "$2" ]]
}

# ---------- 도우미 ----------

have() { command -v "$1" >/dev/null 2>&1; }

# 설정 파일의 값 하나(따옴표를 벗긴다, 없으면 빈 글): toml_get <구역 또는 ""> <키>
toml_get() {
  [[ -f $CONFIG ]] || return 0
  python3 -I - "$CONFIG" "$1" "$2" <<'PY'
import re, sys
path, section, key = sys.argv[1:4]
inside = not section
for line in open(path, encoding="utf-8").read().split("\n"):
    s = line.strip()
    if s.startswith("["):
        inside = s == f"[{section}]"
        continue
    m = re.match(rf"\s*{re.escape(key)}\s*=\s*(.*)", line)
    if inside and m:
        print(m.group(1).split("#")[0].strip().strip('"'))
        break
PY
}

# `~`로 시작하는 경로를 홈으로 편다.
expand() {
  local p=$1
  [[ $p == "~" ]] && p=$HOME
  [[ $p == "~/"* ]] && p="$HOME/${p#\~/}"
  printf '%s' "$p"
}

# 지워도 되는 경로인가: 뿌리($2, 기본은 홈) 아래의 절대 경로이고, 뿌리 자체나 `..`이 든 경로가 아니다.
removable() {
  local p=$1 root=${2:-$HOME}
  [[ -n $root && $root == /?* && $p == "$root/"?* && $p != *"/../"* && $p != *"/.." ]]
}

# 디렉터리나 파일을 지운다(위 조건을 지킬 때만). 아니면 손으로 지울 명령을 알려 준다.
# 레포 안의 빌드 결과처럼 홈 밖일 수 있는 것은 뿌리를 따로 준다: remove_path <경로> [뿌리]
remove_path() {
  local p=$1
  [[ -e $p || -L $p ]] || return 0
  if ! removable "$p" "${2:-$HOME}"; then
    bad "홈 밖이거나 확인할 수 없는 경로라 지우지 않습니다: $p"
    cmd "rm -rf -- '$p'   # 맞는 경로인지 확인하고 직접"
    kept "$p"
    return 1
  fi
  rm -rf -- "$p" && ok "지웠습니다: $p" || {
    bad "지우지 못했습니다: $p"
    kept "$p"
    return 1
  }
}

size_of() { du -sh -- "$1" 2>/dev/null | cut -f1; }

sbx_daemon_running() { have sbx && [[ -S $SOCKET ]] && pgrep -x sbx >/dev/null 2>&1; }

# 지울 때는 이미 떠 있는 데몬만 쓴다(없으면 새로 띄우지 않는다: 샌드박스를 보려고 데몬을 띄우는 것은 걷어 내기와 반대다).
sbx_() { sbx_daemon_running && sbx "$@"; }

# Lucia의 샌드박스 이름: 설정에 적힌 것과, 스크립트들이 만드는 `lucia`, `lucia-…`.
lucia_sandboxes() {
  local configured=() name
  for name in "$(toml_get sandbox name)" "$(toml_get browser sandbox)" "$(toml_get scripts sandbox)"; do
    [[ -n $name ]] && configured+=("$name")
  done
  sbx_ ls -q 2>/dev/null | while read -r name; do
    [[ -z $name ]] && continue
    if [[ $name == lucia || $name == lucia-* ]]; then
      echo "$name"
    else
      for c in "${configured[@]:-}"; do [[ $name == "$c" ]] && echo "$name"; done
    fi
  done | sort -u
}

unit_present() { [[ -f $UNIT_DIR/$1 ]] || systemctl --user cat "$1" >/dev/null 2>&1; }

remove_unit() {
  local unit=$1
  systemctl --user disable "$unit" >/dev/null 2>&1 || true
  rm -f -- "$UNIT_DIR/$unit"
  systemctl --user daemon-reload >/dev/null 2>&1 || true
  systemctl --user reset-failed "$unit" >/dev/null 2>&1 || true
}

# ---------- 단계 ----------

service() {
  step 1 "Lucia 데몬(luciad.service)"
  if ! unit_present luciad.service; then
    ok "서비스가 없습니다."
    return 0
  fi
  local state
  state=$(systemctl --user is-active luciad.service 2>/dev/null || true)
  found "luciad.service (${state:-알 수 없음})"
  [[ $CHECK_ONLY == 1 ]] && return 0
  note "멈추면 하던 대화와 정기 실행(브리핑, 후속 조치, 감시)이 모두 멈춥니다."
  if ask "서비스를 멈추고 지울까요?"; then
    systemctl --user stop luciad.service >/dev/null 2>&1 || true
    remove_unit luciad.service
    ok "luciad.service를 멈추고 지웠습니다."
  else
    kept "luciad.service"
    note "서비스가 도는 동안에는 아래 단계에서 지운 것을 데몬이 다시 만들 수 있습니다."
  fi
}

sandboxes() {
  step 2 "샌드박스"
  if ! have sbx; then
    ok "sbx가 없습니다."
    return 0
  fi
  if ! sbx_daemon_running; then
    found "sbx 데몬이 떠 있지 않아 샌드박스를 볼 수 없습니다."
    note "남은 샌드박스를 지우려면 데몬을 띄운 뒤 다시 돌립니다: sg kvm -c 'sbx ls'"
    kept "샌드박스(확인하지 못함)"
    return 0
  fi
  local boxes=()
  mapfile -t boxes < <(lucia_sandboxes)
  if [[ ${#boxes[@]} -eq 0 ]]; then
    ok "Lucia의 샌드박스가 없습니다."
    return 0
  fi
  for b in "${boxes[@]}"; do found "$b"; done
  [[ $CHECK_ONLY == 1 ]] && return 0
  note "지우면 안에 든 것이 사라집니다: 상주 대화의 기록, 로그인된 브라우저 프로필(쿠키), 설치한 패키지."
  note "다른 이름의 샌드박스는 건드리지 않습니다."
  if ask "위 샌드박스 ${#boxes[@]}개를 지울까요?"; then
    for b in "${boxes[@]}"; do
      if sbx_ rm --force "$b" >/dev/null 2>&1; then ok "지웠습니다: $b"; else
        bad "지우지 못했습니다: $b"
        kept "샌드박스 $b"
      fi
    done
    note "샌드박스에 걸어 둔 네트워크 규칙이 남았는지는 sbx policy ls 로 봅니다."
  else
    for b in "${boxes[@]}"; do kept "샌드박스 $b"; done
  fi
}

sbx_shared() {
  step 3 "sbx 공용 상태(다른 샌드박스와 함께 쓰는 것)"
  if ! sbx_daemon_running; then
    ok "건너뜁니다(sbx 데몬이 없습니다)."
    return 0
  fi
  local has_claude=0
  sbx_ secret ls 2>/dev/null | grep -q 'anthropic' && has_claude=1
  if [[ $has_claude == 1 ]]; then found "Claude 로그인(sbx 시크릿 anthropic)"; else ok "Claude 로그인 시크릿이 없습니다."; fi
  found "전역 네트워크 정책, Docker 로그인"
  note "셋 다 이 서버의 다른 sbx 샌드박스도 함께 씁니다. Lucia만 쓰던 서버가 아니면 그대로 둡니다."
  [[ $CHECK_ONLY == 1 ]] && return 0
  if [[ $has_claude == 1 ]]; then
    if ask "Claude 로그인 시크릿을 지울까요? (다른 샌드박스의 Claude도 로그아웃됩니다)"; then
      sbx_ secret rm anthropic -f >/dev/null 2>&1 && ok "anthropic 시크릿을 지웠습니다." || bad "지우지 못했습니다: sbx secret rm anthropic"
    else
      kept "sbx 시크릿 anthropic"
    fi
  fi
  if ask "전역 네트워크 정책을 처음 상태로 되돌릴까요? (sbx policy reset: 돌고 있는 샌드박스가 모두 멈춥니다)"; then
    sbx_ policy reset --force >/dev/null 2>&1 && ok "정책을 되돌렸습니다." || bad "되돌리지 못했습니다: sbx policy reset"
  fi
  if ask "Docker에서 로그아웃할까요? (sbx logout: 돌고 있는 샌드박스가 모두 멈춥니다)"; then
    sbx logout >/dev/null 2>&1 && ok "로그아웃했습니다." || bad "로그아웃하지 못했습니다: sbx logout"
  fi
}

sbx_unit() {
  step 4 "sbx 데몬 유닛(lucia-sbx.service)"
  if ! unit_present lucia-sbx.service; then
    ok "유닛이 없습니다."
    return 0
  fi
  local state
  state=$(systemctl --user is-active lucia-sbx.service 2>/dev/null || true)
  found "lucia-sbx.service (${state:-알 수 없음})"
  [[ $CHECK_ONLY == 1 ]] && return 0
  note "이 유닛은 부팅 때 sbx 데몬을 띄웁니다. 지우면 자동으로 뜨지 않습니다(sbx 명령을 쓰면 그때 뜹니다)."
  if ! ask "유닛을 지울까요?"; then
    kept "lucia-sbx.service"
    return 0
  fi
  if [[ $state == active ]]; then
    note "지금 이 유닛이 sbx 데몬을 돌리고 있습니다. 멈추면 이 서버의 모든 sbx 샌드박스가 멈춥니다."
    if ask "데몬도 지금 멈출까요? (아니오: 유닛만 지우고 데몬은 다음 재부팅까지 둡니다)"; then
      systemctl --user stop lucia-sbx.service >/dev/null 2>&1 || true
    fi
  fi
  remove_unit lucia-sbx.service
  ok "lucia-sbx.service를 지웠습니다."
}

binary() {
  step 5 "바이너리"
  if [[ ! -e $BIN_DIR ]]; then
    ok "$BIN_DIR 이(가) 없습니다."
    return 0
  fi
  found "$BIN_DIR ($(size_of "$BIN_DIR"))"
  [[ $CHECK_ONLY == 1 ]] && return 0
  if ask "지울까요?"; then remove_path "$BIN_DIR"; else kept "$BIN_DIR"; fi
}

config_and_credentials() {
  step 6 "설정과 자격 증명"
  if [[ ! -e $CONFIG_DIR ]]; then
    ok "$CONFIG_DIR 이(가) 없습니다."
    return 0
  fi
  local creds
  creds=$(find "$CREDENTIALS" -maxdepth 1 -name '*.cred' 2>/dev/null | wc -l)
  found "$CONFIG_DIR (설정 파일, 암호화한 자격 증명 ${creds}개)"
  if [[ $CREDENTIALS != "$CONFIG_DIR"/* && -e $CREDENTIALS ]]; then found "$CREDENTIALS (자격 증명, 설정 디렉터리 밖)"; fi
  [[ $CHECK_ONLY == 1 ]] && return 0
  note "여기서 지워도 바깥 서비스의 토큰은 살아 있습니다. 쓰지 않을 것이면 그쪽에서 거둡니다:"
  note "Discord 개발자 포털(봇 토큰 재발급·봇 삭제), Google 계정의 앱 접근 권한, Firebase 서비스 계정 키, OpenAI API 키."
  if ask "설정과 자격 증명을 지울까요?"; then
    [[ $CREDENTIALS != "$CONFIG_DIR"/* ]] && remove_path "$CREDENTIALS"
    remove_path "$CONFIG_DIR"
  else
    kept "$CONFIG_DIR"
  fi
}

data() {
  step 7 "데이터"
  local dirs=() d
  for d in "$DATA_DIR" "$HOME/.local/share/lucia-dev"; do
    [[ -e $d ]] && dirs+=("$d")
  done
  if [[ ${#dirs[@]} -eq 0 ]]; then
    ok "데이터 디렉터리가 없습니다."
    return 0
  fi
  for d in "${dirs[@]}"; do found "$d ($(size_of "$d"))"; done
  [[ $CHECK_ONLY == 1 ]] && return 0
  note "기억, 대화 기록, 받은 파일, 프로필 그림, 저장소 백업, 세션 기록 백업이 들어 있습니다. 지우면 되돌릴 수 없습니다."
  for d in "${dirs[@]}"; do
    if ! ask "$d 을(를) 지울까요?"; then
      kept "$d"
      continue
    fi
    if removable "$d" && ask "지우기 전에 한 파일로 묶어 둘까요? (홈에 tar.gz로 남깁니다. 메일 내용과 기억이 들어 있으니 잘 보관하세요)"; then
      local archive
      archive="$HOME/$(basename "$d")-data-$(date +%Y%m%d-%H%M%S).tar.gz"
      if (umask 077 && tar -czf "$archive" -C "$(dirname "$d")" "$(basename "$d")"); then
        ok "묶었습니다: $archive ($(size_of "$archive"))"
      else
        bad "묶지 못했습니다. 이 디렉터리는 지우지 않습니다."
        rm -f -- "$archive"
        kept "$d"
        continue
      fi
    fi
    if ask_word "$d 을(를) 지웁니다." "삭제"; then remove_path "$d"; else
      note "지우지 않았습니다."
      kept "$d"
    fi
  done
}

extras() {
  step 8 "그 밖"
  # 밖에서 접속하려고 띄운 라벨 컨테이너(deploy/traefik).
  if have docker && docker ps -a --format '{{.Names}}' 2>/dev/null | grep -qx lucia-route; then
    found "Traefik 라벨 컨테이너(lucia-route)"
    if ask "내릴까요? (밖에서 들어오는 길이 닫힙니다)"; then
      docker compose -f "$REPO/deploy/traefik/docker-compose.yml" down >/dev/null 2>&1 ||
        docker rm -f lucia-route >/dev/null 2>&1
      docker ps -a --format '{{.Names}}' 2>/dev/null | grep -qx lucia-route && bad "내리지 못했습니다: docker rm -f lucia-route" || ok "내렸습니다."
    elif [[ $CHECK_ONLY == 0 ]]; then
      kept "컨테이너 lucia-route"
    fi
  fi

  # 빌드한 것(레포 안).
  local built=() p
  for p in "$REPO/target" "$REPO/web/admin/node_modules" "$REPO/web/admin/dist"; do
    [[ -e $p ]] && built+=("$p")
  done
  if [[ ${#built[@]} -gt 0 ]]; then
    for p in "${built[@]}"; do found "$p ($(size_of "$p"))"; done
    if ask "레포 안의 빌드 결과를 지울까요?"; then
      for p in "${built[@]}"; do remove_path "$p" "$REPO"; done
    fi
  fi

  # 로그인하지 않아도 사용자 서비스가 뜨게 하는 설정(다른 사용자 서비스도 쓴다).
  if [[ "$(loginctl show-user "$USER" -p Linger --value 2>/dev/null)" == yes ]]; then
    found "linger 켜짐"
    note "이 사용자의 다른 systemd 서비스도 이것으로 부팅 때 뜹니다. Lucia만 쓰던 것이면 끕니다."
    if ask "끌까요? (loginctl disable-linger $USER)"; then
      loginctl disable-linger "$USER" && ok "linger 끔" || bad "끄지 못했습니다."
    fi
  fi
  [[ $CHECK_ONLY == 1 ]] && return 0
  return 0
}

summary() {
  echo
  if [[ $CHECK_ONLY == 1 ]]; then
    echo "지금 상태만 봤습니다. 지우려면: deploy/uninstall.sh"
    return 0
  fi
  if [[ ${#LEFT[@]} -gt 0 ]]; then
    echo "  ${B}남겨 둔 것${N}"
    for item in "${LEFT[@]}"; do echo "    - $item"; done
    echo
  fi
  # 받아 쓰는 설치에서는 이 스크립트가 든 자리(~/.local/lib/lucia/release)가 "바이너리" 단계에서 함께 지워진다.
  local repo_line="레포           $REPO   (지우려면: rm -rf '$REPO')"
  [[ $REPO == "$BIN_DIR"/* ]] && repo_line="레포           없음(받아 쓰는 설치: 풀어 둔 릴리스는 바이너리와 함께 지워졌거나 $BIN_DIR 에 있습니다)"
  cat <<EOF
  ${B}끝났습니다.${N} 이 스크립트가 건드리지 않은 것:
    $repo_line
    도구           sbx, Rust, Node.js (다른 데 쓰지 않으면 각자의 방법으로 지웁니다)
    바깥의 토큰     Discord 봇, Google 동의, Firebase 키, OpenAI 키 (그쪽 화면에서 거둡니다)
EOF
}

main() {
  case "${1:-}" in
    --check) CHECK_ONLY=1 ;;
    -h | --help)
      sed -n '2,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    "") ;;
    *) die "모르는 인자: $1 (--check, --help)" ;;
  esac
  [[ $EUID -eq 0 ]] && die "root로 돌리지 않습니다. Lucia를 설치한 사용자로 실행하세요."
  have python3 || die "python3가 필요합니다(설정 파일을 읽습니다)."

  # 설정을 지우기 전에, 거기 적힌 위치를 먼저 읽어 둔다.
  DATA_DIR=$(expand "$(toml_get "" data_dir)")
  CREDENTIALS=$(expand "$(toml_get "" credentials_dir)")
  DATA_DIR=${DATA_DIR:-$HOME/.local/share/lucia}
  CREDENTIALS=${CREDENTIALS:-$CONFIG_DIR/credentials}

  printf '%sLucia 걷어 내기%s  %s\n' "$B" "$N" "$REPO"
  if [[ $CHECK_ONLY == 1 ]]; then
    echo "지금 상태만 봅니다. 아무것도 지우지 않습니다."
  else
    # LUCIA_UNINSTALL_ALLOW_PIPE=1은 시험용이다(답을 표준 입력으로 넣는다).
    [[ -t 0 || ${LUCIA_UNINSTALL_ALLOW_PIPE:-0} == 1 ]] ||
      die "묻고 답하는 스크립트입니다. 터미널에서 돌려 주세요(상태만 보려면 --check)."
    echo "묶음마다 묻습니다. 답하지 않고 Enter를 치면 지우지 않습니다. 멈추려면 Ctrl+C."
  fi
  service
  sandboxes
  sbx_shared
  sbx_unit
  binary
  config_and_credentials
  data
  extras
  summary
}

# 시험에서 함수만 가져다 쓸 수 있게, 직접 돌릴 때만 시작한다.
if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
  main "$@"
fi
