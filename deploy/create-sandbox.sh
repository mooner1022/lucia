#!/usr/bin/env bash
# 상주 샌드박스를 만든다(MVP 2번 조각). 이미 있으면 아무것도 하지 않는다.
#
# - 작업 공간: 호스트와 VM에서 같은 경로(쓰기 가능)
# - 읽기 전용 마운트: plugin(훅) → /opt/lucia/plugin, 캐릭터 → /opt/lucia/character
#   (VM의 root도 못 뚫는다. PoC P2)
# - 메모리 상한, 스킬 공유 끔, sbx MCP 게이트웨이 끔
# - 킷이 여는 계정 쪽·업데이트·패키지 주소는 거부 규칙으로 막는다(PoC P4 "킷 허용 규칙 좁히기")
# - 데몬 MCP 포트와, 데몬의 출구 프록시 포트(결정 0058·0059)만 이 샌드박스에 허용한다. 출구 프록시는 통행증이
#   있는 실행(사람이 대화 중인 실행의 쉘)만 지나가게 하고 차단 목록과 사설·내부 주소를 거른다. 데몬도 뜰 때 이
#   규칙을 설정에 맞춘다([sandbox] native_tools, [scripts] net). 브라우저 프록시 포트는 허용하지 않는다.
#
# 받아 쓰는 설치(결정 0065: 레포 자리에 빌드해 둔 luciad와 VERSION이 있다)에서는 캐릭터를 설정 디렉터리
# (~/.config/lucia/character)에 두고 붙인다. 없을 때만 기본 캐릭터를 복사해 넣으므로, 관리 페이지에서 고친 것이
# 업데이트로 덮이지 않는다. plugin은 풀어 둔 릴리스의 것을 그대로 붙인다(업데이트가 그 자리의 내용만 바꾼다).
#
# 사용법: deploy/create-sandbox.sh
# 환경 변수: LUCIA_SANDBOX(기본 lucia), LUCIA_WORKSPACE, LUCIA_MCP_PORT(기본 47810), LUCIA_SANDBOX_MEMORY(기본 4g),
#   LUCIA_PROXY_PORT(기본 47812, 설정 [scripts] proxy_bind의 포트. 0이면 열지 않는다)
set -euo pipefail

NAME=${LUCIA_SANDBOX:-lucia}
REPO=$(cd "$(dirname "$0")/.." && pwd)
WORKSPACE=${LUCIA_WORKSPACE:-$HOME/.local/share/lucia/workspace}
PORT=${LUCIA_MCP_PORT:-47810}
MEMORY=${LUCIA_SANDBOX_MEMORY:-4g}
PROXY_PORT=${LUCIA_PROXY_PORT:-47812}
DENY=(mcp-proxy.anthropic.com bridge.claudeusercontent.com claude.com downloads.claude.ai deb.debian.org dhi.io)

# sbx CLI는 데몬이 없으면 자동으로 띄우고, 데몬은 이 셸의 그룹을 물려받는다(PoC P1).
socket=$HOME/.local/state/sandboxes/sandboxes/sandboxd/sandboxd.sock
case " $(id -nG) " in
  *" kvm "*) ;;
  *)
    if ! { [[ -S $socket ]] && pgrep -x sbx >/dev/null; }; then
      echo "kvm 그룹이 없는 셸이고 sbx 데몬도 없다. 먼저: sg kvm -c 'sbx ls'" >&2
      exit 2
    fi
    ;;
esac

if sbx ls -q | grep -qx "$NAME"; then
  echo "샌드박스 $NAME 이미 있음. 다시 만들려면: sbx rm --force $NAME (세션 기록이 지워진다)"
  exit 0
fi

mkdir -p "$WORKSPACE"
args=(create --name "$NAME" --memory "$MEMORY" --skills off --static-mcp none --no-progress)
for host in "${DENY[@]}"; do
  args+=(--deny-network "$host")
done
PLUGIN_DIR="${LUCIA_PLUGIN_DIR:-$REPO/plugin}"
if [[ -n ${LUCIA_CHARACTER_DIR:-} ]]; then
  CHARACTER_DIR=$LUCIA_CHARACTER_DIR
elif [[ -x $REPO/luciad && -f $REPO/VERSION ]]; then
  CHARACTER_DIR="$HOME/.config/lucia/character"
  mkdir -p "$CHARACTER_DIR"
  [[ -f $CHARACTER_DIR/character.md ]] || cp -- "$REPO/character/character.md" "$CHARACTER_DIR/character.md"
else
  CHARACTER_DIR="$REPO/character"
fi
sbx "${args[@]}" claude "$WORKSPACE" \
  "$PLUGIN_DIR:/opt/lucia/plugin:ro" \
  "$CHARACTER_DIR:/opt/lucia/character:ro"

sbx policy allow network "localhost:$PORT" --sandbox "$NAME"
if [[ $PROXY_PORT != 0 ]]; then
  sbx policy allow network "localhost:$PROXY_PORT" --sandbox "$NAME"
fi

echo "== 확인"
for host in api.anthropic.com:443 "localhost:$PORT" mcp-proxy.anthropic.com:443 downloads.claude.ai:443 example.com:443; do
  printf '  %-34s ' "$host"
  # 거부된 주소는 실패 코드로 끝나므로 여기서는 결과만 보여 준다.
  { sbx policy check network "$host" --sandbox "$NAME" || true; } | head -1
done
sbx exec "$NAME" ls /opt/lucia/plugin/.claude-plugin/plugin.json /opt/lucia/character/character.md
if sbx exec "$NAME" touch /opt/lucia/plugin/x 2>/dev/null; then
  echo "경고: plugin 마운트에 쓸 수 있다(읽기 전용이어야 한다)" >&2
  exit 1
fi
echo "  plugin 마운트 읽기 전용 확인"
