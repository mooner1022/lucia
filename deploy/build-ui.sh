#!/usr/bin/env bash
# 관리 화면(결정 0061) 빌드: 의존성 설치 → 타입 검사 → 시험(기능 보존, 화면 규칙, 표기) → 빌드.
# 하나라도 실패하면 멈춘다(낡거나 깨진 화면이 바이너리에 들어가지 않게). 만든 것은 web/admin/dist에 있고,
# 이어지는 cargo build가 그것을 luciad 바이너리에 넣는다.
#
#   deploy/build-ui.sh            # 전부
#   deploy/build-ui.sh --no-test  # 시험을 건너뛴다(개발 중에만)
#
# Node는 빌드할 때만 쓴다. PATH에 없으면 ~/.local/opt/node/bin(공식 배포판을 푼 곳)을 쓰고,
# 다른 곳에 있으면 LUCIA_NODE_BIN으로 알려 준다.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
NODE_BIN="${LUCIA_NODE_BIN:-$HOME/.local/opt/node/bin}"
if ! command -v node >/dev/null 2>&1; then
  if [[ -x "$NODE_BIN/node" ]]; then
    export PATH="$NODE_BIN:$PATH"
  else
    echo "node를 찾을 수 없다. PATH에 두거나 LUCIA_NODE_BIN으로 알려 준다(공식 배포판: https://nodejs.org/dist/)." >&2
    exit 1
  fi
fi
TEST=1
[[ "${1:-}" == "--no-test" ]] && TEST=0

cd "$REPO/web/admin"
echo "== 화면: 의존성(npm ci)"
npm ci --no-audit --no-fund
if [[ $TEST == 1 ]]; then
  echo "== 화면: 타입 검사"
  npm run --silent check
  echo "== 화면: 시험"
  npm run --silent test
fi
echo "== 화면: 빌드"
npm run --silent build
test -f dist/index.html
