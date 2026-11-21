#!/bin/sh
# 데몬 실행에서 허용한 도구만 통과시킨다(결정 0003·0059). 판단은 gate.py가 한다:
# - 데몬 MCP 도구(mcp__lucia__*)와, 데몬이 이 실행에 열어 준 내장 도구(환경 변수 LUCIA_BUILTIN_TOOLS)만 통과.
#   데몬이 알려 주지 않았으면 WebSearch만이다.
# - Bash 명령 가운데 되돌릴 수 없이 바깥에 영향을 줄 수 있는 것(git push, 릴리스·배포·게시, 바깥으로 보내는 쓰기 요청)은
#   실행 전에 데몬에 물어 사용자의 승인을 기다린다.
# 나머지는 종료 코드 2로 막는다. 실행 조합(--restricted, --tools 목록)으로 이미 막혀 있지만, 조합이 잘못 바뀌어도
# 막히게 하는 이중 방어다. 입력을 해석하지 못하거나 gate.py가 돌지 못하면 막는다(fail closed).
#
# 내장 도구 허용의 경과:
#   2026-11-16 WebSearch(결정 0003, 사용자 지시로 권한 완화).
#   2026-11-20 사람이 대화 중인 실행에 Bash·Read·Write·Edit·Glob·Grep(결정 0059, 사용자 지시: VM 안에서
#   Claude Code의 온전한 성능). 어느 실행에 무엇을 여는지는 데몬이 정해 환경 변수로 알려 준다.
python3 -I "$(dirname "$0")/gate.py"
code=$?
case "$code" in
  0) exit 0 ;;
  2) exit 2 ;;
esac
echo "lucia: 도구 검사를 하지 못했다(코드 $code). 막는다." >&2
exit 2
