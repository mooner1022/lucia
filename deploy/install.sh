#!/usr/bin/env bash
# Lucia 운영 배포: 릴리스 빌드 → 운영 저장소 백업 → 고정 경로에 바이너리 설치 → 사용자 유닛 설치·재시작.
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
# 재시작하면 실행 중인 작업은 실패로 닫힌다(결정 0014). 하는 중인 작업이 있으면 멈춘다(--force로 무시).
if [[ $RESTART == 1 && $FORCE == 0 && -f "$DB" ]]; then
  RUNNING="$(sqlite3 "$DB" "SELECT count(*) FROM tasks WHERE status = 'running'" 2>/dev/null || echo 0)"
  if [[ "$RUNNING" != "0" ]]; then
    echo "하는 중인 작업이 ${RUNNING}개 있어 멈춥니다. 끝난 뒤 다시 하거나 --force를 붙이세요." >&2
    exit 1
  fi
fi
# 관리 화면(결정 0061)을 먼저 빌드한다. 바이너리가 그것을 품으므로 화면과 데몬의 판이 어긋나지 않는다.
deploy/build-ui.sh
echo "== 릴리스 빌드"
cargo build --release -p luciad
# 새 바이너리에 화면이 온전히 들어갔는지 본다(틀이 가리키는 파일이 모두 있는지). 아니면 설치하지 않는다.
target/release/luciad ui-check

if [[ -f "$DB" ]]; then
  STAMP="$(date +%Y-%m-%d-%H%M%S)"
  BACKUP="$DATA_DIR/backup/lucia.sqlite.$STAMP"
  mkdir -p "$DATA_DIR/backup"
  echo "== 저장소 백업: $BACKUP"
  # 데몬이 돌고 있어도 일관된 사본을 만든다.
  sqlite3 "$DB" ".backup '$BACKUP'"
  # 최근 20개만 남긴다.
  ls -1t "$DATA_DIR"/backup/lucia.sqlite.* 2>/dev/null | tail -n +21 | xargs -r rm --
fi

echo "== 바이너리 설치: $BIN_DIR/luciad"
mkdir -p "$BIN_DIR"
install -m 0755 target/release/luciad "$BIN_DIR/luciad.new"
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
    sqlite3 "$DB" "SELECT (SELECT count(*) FROM inbox) + (SELECT count(*) FROM settings WHERE key = 'runtime.running')" 2>/dev/null || echo 0
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
