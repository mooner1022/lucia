#!/usr/bin/env bash
# 화면 검사를 한 번에: 화면 빌드 → 시험 데이터 서버(실제 라우터) 띄우기 → 브라우저 검사 → 서버 끄기.
#   web/admin/e2e/run.sh [check.mjs에 줄 인자…]      예: web/admin/e2e/run.sh --sets stress,full --shots
#   web/admin/e2e/run.sh --flows [flows.mjs에 줄 인자…]   배치 검사 대신 조작 흐름 검사(로그인·폼·창·원격 화면 …)를 돌린다
# 서버는 데몬의 시험 바이너리다(cargo test -p luciad serve_ui_fixtures). 처음에는 빌드에 시간이 걸린다.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
ADDR="${LUCIA_UI_FIXTURE_ADDR:-127.0.0.1:47890}"
NODE_BIN="${LUCIA_NODE_BIN:-$HOME/.local/opt/node/bin}"
command -v node >/dev/null 2>&1 || export PATH="$NODE_BIN:$PATH"

# 첫 인자가 --flows면 check.mjs 대신 flows.mjs를 돌린다(나머지 인자는 그대로 넘긴다).
SCRIPT=check.mjs
if [ "${1:-}" = "--flows" ]; then
  SCRIPT=flows.mjs
  shift
fi

cd "$REPO/web/admin"
npm run --silent build >/dev/null
cd "$REPO"
# 먼저 빌드만 한다(서버가 뜨기를 기다리는 시간에 빌드 시간이 섞이지 않게).
cargo test -p luciad serve_ui_fixtures --no-run 2>&1 | tail -1
LOG="$(mktemp)"
LUCIA_UI_FIXTURE_ADDR="$ADDR" cargo test -p luciad serve_ui_fixtures -- --ignored --nocapture >"$LOG" 2>&1 &
RUNNER=$!
stop() {
  # cargo와 그것이 띄운 시험 바이너리를 끝낸다(이름으로 찾아 끝내지 않는다: 이 스크립트를 부른 셸까지 걸린다).
  pkill -P "$RUNNER" 2>/dev/null || true
  kill "$RUNNER" 2>/dev/null || true
  rm -f "$LOG"
}
trap stop EXIT
for _ in $(seq 1 120); do
  grep -q "화면 검사용 서버" "$LOG" 2>/dev/null && break
  sleep 0.5
done
grep -q "화면 검사용 서버" "$LOG" || { echo "시험 데이터 서버가 뜨지 않았다:" >&2; tail -20 "$LOG" >&2; exit 2; }
cd "$REPO/web/admin"
node "e2e/$SCRIPT" --base "http://$ADDR" "$@"
