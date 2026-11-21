#!/usr/bin/env bash
# PoC P3: 호스트 재부팅 뒤 sbx 데몬·샌드박스·세션이 복구되는지 확인한다.
#
# 상태(2026-11-16): 재부팅 시험은 건너뛰었다. 호스트에 다른 서비스가 있어 시험만을 위해
# 재부팅할 수 없다. 유지보수 등으로 재부팅하게 되면 이 스크립트를 돌린다(poc/RESULTS.md P3).
#
# 재부팅 전 상태(2026-11-16, P4에서 만든 세션):
#   샌드박스 lucia-poc, 세션 ID와 기억한 단어는 poc/p3/.env에 둔다(레포에 올리지 않는다):
#     LUCIA_POC_SESSION=<세션 ID>
#     LUCIA_POC_WORD=<기억하라고 한 단어>
#   (샌드박스 자동 정지 후 재개는 재부팅 전에 이미 확인함)
#
# 사용법(재부팅 후 kvm 그룹이 있는 로그인 셸에서): poc/p3/after_reboot.sh
# 모델을 한 번 부른다(haiku).
set -uo pipefail

SANDBOX=lucia-poc
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck disable=SC1091
[[ -f $HERE/.env ]] && . "$HERE/.env"
SESSION=${LUCIA_POC_SESSION:?poc/p3/.env에 LUCIA_POC_SESSION(세션 ID)을 적는다}
WORD=${LUCIA_POC_WORD:?poc/p3/.env에 LUCIA_POC_WORD(기억하라고 한 단어)를 적는다}
WS=$HOME/.local/share/lucia/poc/workspace

echo "== 셸 그룹: $(id -nG)"
# sbx CLI는 데몬이 없으면 자동으로 띄우고, 데몬은 이 셸의 그룹을 물려받는다(P1).
# kvm 그룹 없이 데몬을 띄우면 VM을 못 만들므로, 그 경우에는 sbx를 부르기 전에 멈춘다.
socket=$HOME/.local/state/sandboxes/sandboxes/sandboxd/sandboxd.sock
case " $(id -nG) " in
  *" kvm "*) echo "   kvm 그룹 있음" ;;
  *)
    if [[ -S $socket ]] && pgrep -x sbx >/dev/null; then
      echo "   kvm 그룹 없음. 데몬이 이미 떠 있으므로 계속한다(데몬 GID는 아래에서 확인)."
    else
      echo "   kvm 그룹 없음, 데몬도 없음: 여기서 sbx를 부르면 kvm 없는 데몬이 뜬다."
      echo "   kvm 그룹이 있는 로그인 셸에서 다시 실행하거나, 먼저 sg kvm -c 'sbx ls' 로 데몬을 띄운다."
      exit 2
    fi
    ;;
esac

echo "== sbx 데몬(재부팅 직후에는 아직 없을 수 있음):"
sbx daemon status 2>&1 | sed 's/^/   /'

echo "== 진단(Virtualization 줄은 데몬이 아니라 이 셸의 /dev/kvm 권한이다):"
sbx diagnose 2>&1 | grep -E 'Daemon —|Virtualization|Authentication|Global network policy' | sed 's/^/   /'

echo "== 데몬 프로세스 GID:"
for p in $(pgrep -x sbx); do echo "   PID $p $(grep -E '^Gid' /proc/$p/status)"; done

echo "== 샌드박스 목록:"
sbx ls 2>&1 | sed 's/^/   /'

echo "== 시크릿(Claude 로그인 유지 여부):"
sbx secret ls 2>&1 | sed 's/^/   /'

echo "== 세션 재개:"
start=$(date +%s.%N)
out=$(python3 -c 'import json; print(json.dumps({"type":"user","message":{"role":"user","content":"앞에서 기억하라고 한 단어만 정확히 답하라. 모르면 모름이라고 답하라."}}))' \
  | timeout 240 sbx exec -i -w "$WS" "$SANDBOX" claude -p --input-format stream-json --output-format stream-json \
      --verbose --restricted --strict-mcp-config --tools "" --permission-prompts none --model haiku --resume "$SESSION" 2>&1)
end=$(date +%s.%N)
answer=$(printf '%s\n' "$out" | python3 -I -c '
import json, sys
for line in sys.stdin:
    try:
        d = json.loads(line)
    except json.JSONDecodeError:
        continue
    if d.get("type") == "result":
        print("%s\t%s\t%s" % (d.get("is_error"), d.get("session_id"), d.get("result")))
')
echo "   걸린 시간: $(python3 -c "print(round($end-$start,1))")초"
echo "   결과(is_error, session, text): $answer"
if [[ "$answer" == False*"$SESSION"*"$WORD"* ]]; then
  echo "== P3 판정: 통과(재부팅 뒤 같은 샌드박스에서 이전 세션 재개)"
else
  echo "== P3 판정: 실패"
  exit 1
fi
