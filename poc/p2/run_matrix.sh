#!/usr/bin/env bash
# PoC P2: 권한 모드·훅·플래그 조합별로 Bash 도구 호출이 실제로 실행되는지 확인한다.
#
# 각 경우에 같은 요청(표식 파일을 touch하라)을 보내고, 실행 뒤 VM 안에 표식 파일이 생겼는지 본다.
# 시험용 훅(poc/p2/plugin)은 VM의 /opt/lucia/p2/plugin에 읽기 전용으로 마운트되어 있어야 한다.
# 모델을 실제로 부른다(haiku, 경우마다 1회).
#
# 사용법: poc/p2/run_matrix.sh [SANDBOX] [경우 이름...]
set -uo pipefail

SANDBOX=${1:-lucia-poc}
shift || true
ONLY=("$@")

REPO=$(cd "$(dirname "$0")/../.." && pwd)
RAW="$REPO/poc/raw/p2"
mkdir -p "$RAW"

MARKER=/tmp/lucia-p2-marker
PLUGIN=/opt/lucia/p2/plugin
PROMPT="Use the Bash tool to run exactly this command and nothing else: touch $MARKER . Do not run any other command. Then reply with the single word done."

BASE=(claude -p --input-format stream-json --output-format stream-json --verbose
      --include-hook-events --strict-mcp-config --model haiku --tools Bash)

declare -A CASES=(
  [c1-default]="--setting-sources project"
  [c2-bypass-flag]="--setting-sources project --dangerously-skip-permissions"
  [c3-bypass-flag-hook]="--setting-sources project --dangerously-skip-permissions --plugin-dir $PLUGIN"
  [c4-kit-settings-hook]="--plugin-dir $PLUGIN"
  [c5-prompts-none]="--setting-sources project --permission-prompts none"
  [c6-restricted-hook]="--restricted --permission-prompts none --plugin-dir $PLUGIN"
  [c7-restricted-bypass]="--restricted --dangerously-skip-permissions"
)
ORDER=(c1-default c2-bypass-flag c3-bypass-flag-hook c4-kit-settings-hook c5-prompts-none c6-restricted-hook c7-restricted-bypass)

run_case() {
  local name=$1
  # shellcheck disable=SC2206
  local extra=(${CASES[$name]})
  sbx exec "$SANDBOX" sh -c "rm -f $MARKER /tmp/lucia-p2-guard.log" >/dev/null 2>&1
  printf '%s\n' "$(python3 -c 'import json,sys; print(json.dumps({"type":"user","message":{"role":"user","content":sys.argv[1]}}))' "$PROMPT")" \
    | timeout 240 sbx exec -i "$SANDBOX" "${BASE[@]}" "${extra[@]}" \
      >"$RAW/$name.jsonl" 2>"$RAW/$name.err"
  local code=$?
  local marker guard
  marker=$(sbx exec "$SANDBOX" sh -c "test -e $MARKER && echo CREATED || echo absent" 2>/dev/null)
  guard=$(sbx exec "$SANDBOX" sh -c "wc -l < /tmp/lucia-p2-guard.log 2>/dev/null || echo 0" 2>/dev/null)
  python3 -I "$REPO/poc/p2/summarize.py" "$name" "$RAW/$name.jsonl" "$RAW/$name.err" "$code" "$marker" "$guard"
}

for name in "${ORDER[@]}"; do
  if ((${#ONLY[@]})) && [[ ! " ${ONLY[*]} " == *" $name "* ]]; then
    continue
  fi
  run_case "$name"
done
