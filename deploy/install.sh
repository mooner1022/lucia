#!/usr/bin/env bash
# Lucia 운영 배포: 릴리스 빌드 → 운영 저장소 백업 → 고정 경로에 바이너리 설치 → 사용자 유닛 설치·재시작.
# 받아 쓰는 설치(결정 0065: 이 스크립트 옆에 빌드해 둔 luciad와 VERSION이 있다)에서는 빌드하지 않고 그 바이너리를 설치한다.
#
#   deploy/install.sh            # 화면 빌드·릴리스 빌드·백업·설치·재시작
#   deploy/install.sh --no-restart
#   deploy/install.sh --force       # 하는 중인 작업이 있어도, 하던 일을 기다리지 않고 재시작(작업은 실패로 닫힌다)
#   LUCIA_DEPLOY_WAIT=<초>          # 하던 답·정기 작업이 끝나기를 기다리는 시간(기본 180)
#
# 새 바이너리는 시작할 때 저장소를 마이그레이션한다. 그래서 바꾸기 전에 저장소를 백업한다.
# 데이터 디렉터리가 기본(~/.local/share/lucia)이 아니면 LUCIA_DATA_DIR로 알려 준다.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
DATA_DIR="${LUCIA_DATA_DIR:-$HOME/.local/share/lucia}"
BIN_DIR="$HOME/.local/lib/lucia"
UNIT_DIR="$HOME/.config/systemd/user"
RESTART=1
FORCE=0
for arg in "$@"; do
  case "$arg" in
    --no-restart) RESTART=0 ;;
    --force) FORCE=1 ;;
  esac
done

cd "$REPO"
DB="$DATA_DIR/lucia.sqlite"
# 받아 쓰는 설치인가(빌드해 둔 바이너리가 이 스크립트의 레포 자리에 있다).
PREBUILT=0
[[ -x "$REPO/luciad" && -f "$REPO/VERSION" ]] && PREBUILT=1

# 저장소의 상태 값 하나(running_tasks, inbox, run_in_flight). 저장소를 읽기 전용으로 열고 마이그레이션하지 않는다
# (`luciad store-status`, 결정 0065: sqlite3 명령 없이). 읽지 못하면 빈 글.
store_value() {
  # 읽지 못해도(예전 바이너리, 깨진 저장소) 스크립트를 멈추지 않고 빈 글을 돌려준다. 멈출지는 부른 쪽이 정한다.
  { "$1" store-status --db "$DB" 2>/dev/null || true; } | sed -n "s/^$2=//p"
}
# 하는 중인 작업이 있으면 멈춘다(결정 0014: 재시작하면 실행 중인 작업은 실패로 닫힌다). --force로 무시.
# $2가 strict면 상태를 읽지 못해도 멈춘다(설치 직전의 확인), 아니면 넘어간다(빌드 전에 미리 보는 확인).
refuse_if_running() {
  [[ $RESTART == 1 && $FORCE == 0 && -f "$DB" ]] || return 0
  local running
  running="$(store_value "$1" running_tasks)"
  if [[ -z $running ]]; then
    [[ ${2:-} == strict ]] || return 0
    echo "저장소의 상태를 읽지 못해 멈춥니다($1 store-status). 확인한 뒤 다시 하거나 --force를 붙이세요." >&2
    exit 1
  fi
  if [[ $running != 0 ]]; then
    echo "하는 중인 작업이 ${running}개 있어 멈춥니다. 끝난 뒤 다시 하거나 --force를 붙이세요." >&2
    exit 1
  fi
}

if [[ $PREBUILT == 1 ]]; then
  NEW_BIN="$REPO/luciad"
  echo "== 받아 둔 바이너리: $("$NEW_BIN" --version) ($REPO)"
else
  NEW_BIN="$REPO/target/release/luciad"
  # 빌드에 시간이 걸리므로, 지금 설치된 바이너리로 미리 한 번 본다(예전 바이너리라 못 읽으면 빌드 뒤에 본다).
  [[ -x "$BIN_DIR/luciad" ]] && refuse_if_running "$BIN_DIR/luciad"
  # 관리 화면(결정 0061)을 먼저 빌드한다. 바이너리가 그것을 품으므로 화면과 데몬의 판이 어긋나지 않는다.
  deploy/build-ui.sh
  echo "== 릴리스 빌드"
  cargo build --release -p luciad
fi
# 새 바이너리에 화면이 온전히 들어갔는지 본다(틀이 가리키는 파일이 모두 있는지). 아니면 설치하지 않는다.
"$NEW_BIN" ui-check
refuse_if_running "$NEW_BIN" strict

if [[ -f "$DB" ]]; then
  STAMP="$(date +%Y-%m-%d-%H%M%S)"
  BACKUP="$DATA_DIR/backup/lucia.sqlite.$STAMP"
  mkdir -p "$DATA_DIR/backup"
  echo "== 저장소 백업: $BACKUP"
  # 데몬이 돌고 있어도 일관된 사본을 만든다(읽기 전용으로 열어 SQLite의 온라인 백업으로 뜨고, 사본을 검사한다).
  # 실패하면 여기서 멈춘다(백업 없이 마이그레이션하지 않는다).
  "$NEW_BIN" store-backup --db "$DB" --out "$BACKUP"
  # 최근 20개만 남긴다.
  ls -1t "$DATA_DIR"/backup/lucia.sqlite.* 2>/dev/null | tail -n +21 | xargs -r rm --
fi

echo "== 바이너리 설치: $BIN_DIR/luciad"
mkdir -p "$BIN_DIR"
install -m 0755 "$NEW_BIN" "$BIN_DIR/luciad.new"
# 바로 앞의 바이너리를 남긴다. 되돌리기: mv luciad.prev luciad && systemctl --user restart luciad
# (저장소를 마이그레이션한 배포였다면 위의 백업도 함께 되돌린다.)
[[ -f "$BIN_DIR/luciad" ]] && cp -f "$BIN_DIR/luciad" "$BIN_DIR/luciad.prev"
mv -f "$BIN_DIR/luciad.new" "$BIN_DIR/luciad"

echo "== 사용자 유닛 설치"
mkdir -p "$UNIT_DIR"
install -m 0644 deploy/lucia-sbx.service "$UNIT_DIR/"
# 유닛의 작업 디렉터리를 이 레포의 실제 위치로 맞춘다(레포를 어디에 받았든 되게).
sed "s#^WorkingDirectory=.*#WorkingDirectory=$REPO#" deploy/luciad.service >"$UNIT_DIR/luciad.service"
chmod 0644 "$UNIT_DIR/luciad.service"
systemctl --user daemon-reload
systemctl --user enable luciad.service lucia-sbx.service >/dev/null

if [[ $RESTART == 1 ]]; then
  # 하던 일이 끝나기를 잠깐 기다린다(결정 0050): 받은 메시지(받은 편지함)와 하는 중인 정기 작업이 없을 때 재시작한다.
  # 그래도 끊긴 일은 다시 뜬 데몬이 알리거나(하던 답) 이어서 한다(기다리던 메시지, 정기 작업 한 번).
  WAIT="${LUCIA_DEPLOY_WAIT:-180}"
  busy() {
    local inbox flight
    inbox="$(store_value "$BIN_DIR/luciad" inbox)"
    flight="$(store_value "$BIN_DIR/luciad" run_in_flight)"
    echo $((${inbox:-0} + ${flight:-0}))
  }
  waited=0
  while [[ $FORCE == 0 && -f "$DB" && "$(busy)" != "0" && $waited -lt $WAIT ]]; do
    [[ $waited == 0 ]] && echo "== 하던 일이 끝나기를 기다린다(최대 ${WAIT}초. 기다리지 않으려면 --force)"
    sleep 3
    waited=$((waited + 3))
  done
  if [[ $FORCE == 0 && -f "$DB" && "$(busy)" != "0" ]]; then
    echo "   기다렸지만 끝나지 않았다. 재시작한다(끊긴 일은 다시 뜬 뒤 알리거나 이어서 한다)."
  fi
  echo "== 재시작"
  # 유닛의 시작 횟수 제한(1시간 5번)은 충돌로 거듭 죽는 것을 막으려는 것이다. 배포 재시작은 세지 않게
  # 카운터를 비운다(systemctl reset-failed는 재시작 횟수 카운터도 비운다). 2026-11-17 하루 여러 번 배포하다
  # 제한에 걸려 서비스가 멈춘 적이 있다.
  systemctl --user reset-failed luciad.service 2>/dev/null || true
  systemctl --user restart luciad.service
  sleep 3
  systemctl --user --no-pager status luciad.service | head -n 5
fi
