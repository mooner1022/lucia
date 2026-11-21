#!/bin/sh
# PoC P2: 표식(lucia-p2-marker)이 든 Bash 명령을 막는다. 종료 코드 2 = 차단, stderr가 사유.
input=$(cat)
echo "$(date -u +%FT%TZ) guard invoked" >> /tmp/lucia-p2-guard.log
case "$input" in
  *lucia-p2-marker*)
    echo "blocked by lucia-p2-guard (PoC P2)" >&2
    exit 2
    ;;
esac
exit 0
