#!/usr/bin/env bash
# PoC P7: 데몬 실행 조합(--restricted)에서 플래그가 실제로 동작하는지 확인한다.
# 모델을 실제로 부른다(haiku, 경우마다 1회). 원본 기록은 poc/raw/p7/.
#
# 전제: 샌드박스에 <레포>/poc/p2가 /opt/lucia/p2로 읽기 전용 마운트되어 있고,
#       그 안에 p7-append.md가 있다.
# 사용법: poc/p7/run_flags.sh [SANDBOX] [경우 이름...]
set -uo pipefail

SANDBOX=${1:-lucia-poc}
shift || true
ONLY=("$@")

REPO=$(cd "$(dirname "$0")/../.." && pwd)
RAW="$REPO/poc/raw/p7"
mkdir -p "$RAW"
WS=$HOME/.local/share/lucia/poc/workspace

BASE=(claude -p --input-format stream-json --output-format stream-json --verbose
      --restricted --strict-mcp-config --model haiku --permission-prompts none)

SCHEMA='{"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"],"additionalProperties":false}'

declare -A ARGS PROMPTS
ARGS[max-turns]="--tools Read --max-turns 1"
PROMPTS[max-turns]="Use the Read tool to read /etc/hostname, then tell me its content."
ARGS[json-schema]="--tools ''"
PROMPTS[json-schema]="What is 2+2? Answer in the required structured format."
ARGS[max-budget]="--tools '' --max-budget-usd 0.000001"
PROMPTS[max-budget]="Write a 300-word story about a whale."
ARGS[append-file]="--tools '' --append-system-prompt-file /opt/lucia/p2/p7-append.md"
PROMPTS[append-file]="Say hello."
ARGS[snapshot-off]="--tools '' --system-prompt-snapshot off"
PROMPTS[snapshot-off]="Reply with exactly: pong"
ORDER=(max-turns json-schema max-budget append-file snapshot-off)

run_case() {
  local name=$1
  local -a extra
  eval "extra=(${ARGS[$name]})"
  if [[ $name == json-schema ]]; then extra+=(--json-schema "$SCHEMA"); fi
  python3 -c 'import json,sys; print(json.dumps({"type":"user","message":{"role":"user","content":sys.argv[1]}}))' "${PROMPTS[$name]}" \
    | timeout 240 sbx exec -i -w "$WS" "$SANDBOX" "${BASE[@]}" "${extra[@]}" \
      >"$RAW/$name.jsonl" 2>"$RAW/$name.err"
  local code=$?
  python3 -I - "$name" "$RAW/$name.jsonl" "$RAW/$name.err" "$code" <<'EOF'
import json, sys
name, jsonl, err, code = sys.argv[1:5]
result, init = {}, {}
for line in open(jsonl, encoding="utf-8"):
    try:
        d = json.loads(line)
    except json.JSONDecodeError:
        continue
    if d.get("type") == "result":
        result = d
    if d.get("type") == "system" and d.get("subtype") == "init":
        init = d
stderr = open(err, encoding="utf-8").read().strip().replace("\n", " | ")[:300]
print(f"== {name}  exit={code}")
if init:
    print(f"   init: permissionMode={init.get('permissionMode')} tools={init.get('tools')}")
if result:
    keys = ("subtype", "is_error", "num_turns", "terminal_reason", "stop_reason", "total_cost_usd")
    print("   result:", {k: result.get(k) for k in keys})
    print("   result.text:", repr(str(result.get("result"))[:160]))
    extra = {k: result[k] for k in result if "struct" in k.lower() or "schema" in k.lower() or "budget" in k.lower()}
    if extra:
        print("   result.extra:", json.dumps(extra, ensure_ascii=False)[:300])
if stderr:
    print("   stderr:", stderr)
EOF
}

for name in "${ORDER[@]}"; do
  if ((${#ONLY[@]})) && [[ ! " ${ONLY[*]} " == *" $name "* ]]; then
    continue
  fi
  run_case "$name"
done
