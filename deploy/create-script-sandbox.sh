#!/usr/bin/env bash
# 스크립트 샌드박스를 만든다(결정 0025): Lucia가 쓴 Python 코드(code_run, 스킬 스크립트, 반복 루틴)를 돌리는 곳.
#
# - shell 킷, 작업 공간 없음, 마운트 없음. 코드와 입력은 데몬이 `sbx exec -i`의 표준 입력으로 넣고, 도구 호출과
#   결과도 그 표준 입출력으로 오간다. 샌드박스 안에는 남는 것이 없어도 된다(상태는 데몬 저장소에 있다).
# - 네트워크: **허용 규칙은 데몬의 스크립트 출구 프록시 포트 하나뿐이다**(전역 정책 default-deny-all, 결정 0058).
#   그 프록시는 통행증이 있는 실행(사람이 대화 중인 code_run)만 지나가게 하고, 차단 목록과 사설·내부 주소를 거른다.
#   통행증이 없는 스크립트(루틴 등)가 바깥과 닿는 길은 데몬이 다리로 주는 도구뿐이다. sbx 정책으로 바깥을 직접 열지
#   않는다(이름으로 풀린 사설 IP를 막지 못한다). Lucia가 패키지를 설치하는 동안만(lib_install, 결정 0047) 데몬이
#   저장소 호스트를 열었다 닫는다. 데몬 MCP·브라우저 프록시·관리 페이지 포트와 계정 쪽 이름은 이름으로 한 번 더 막는다.
# - 상주 샌드박스(lucia)·브라우저 샌드박스(lucia-browser)와 따로 둔다: 코드가 상주 VM의 설정(훅의 이중 방어)이나
#   브라우저 프로필(쿠키)에 닿지 않게.
#
# 사용: deploy/create-script-sandbox.sh
#   LUCIA_SCRIPT_SANDBOX(기본 lucia-script), LUCIA_SCRIPT_MEMORY(기본 1g),
#   LUCIA_SCRIPT_PROXY_PORT(기본 47812, 설정 [scripts] proxy_bind의 포트. 0이면 열지 않는다: [scripts] net = false일 때)
#   이미 있는 샌드박스의 네트워크 규칙만 다시 걸려면: deploy/create-script-sandbox.sh --policy-only
set -euo pipefail

NAME=${LUCIA_SCRIPT_SANDBOX:-lucia-script}
MEMORY=${LUCIA_SCRIPT_MEMORY:-1g}
PROXY_PORT=${LUCIA_SCRIPT_PROXY_PORT:-47812}
DENY=(api.anthropic.com '*.anthropic.com' claude.com '*.claude.com' claude.ai '*.claude.ai'
  bridge.claudeusercontent.com '*.claudeusercontent.com'
  localhost:47810 localhost:47811 localhost:47820 localhost:47821 localhost:8780)

case "$NAME" in
  lucia | lucia-poc | lucia-dev | lucia-browser | lucia-browser-dev)
    echo "다른 용도의 샌드박스 이름($NAME)은 쓸 수 없다" >&2
    exit 2
    ;;
esac

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

apply_policy() {
  local deny id
  deny=$(IFS=,; echo "${DENY[*]}")
  sbx policy deny network --sandbox "$NAME" "$deny" >/dev/null 2>&1 || true
  # 이 샌드박스에 걸린 허용 규칙은 출구 프록시 포트 하나만 남기고 모두 지운다(설치 때 잠깐 연 저장소가 남았을 때 등).
  # 데몬도 뜰 때 같은 일을 한다(crates/luciad/src/packages.rs).
  local kept
  kept=$(sbx policy ls "$NAME" --json | python3 -I -c '
import json, sys
name, keep = sys.argv[1], "localhost:" + sys.argv[2]
kept = False
for r in json.load(sys.stdin)["rules"]:
    if (r.get("scope") == "sandbox:" + name and r.get("resource_type") == "network"
            and r.get("decision") == "allow" and r.get("editable")):
        if r.get("resources") == [keep] and not kept and sys.argv[2] != "0":
            kept = True
        else:
            print(r["id"])
print("kept" if kept else "none")' "$NAME" "$PROXY_PORT")
  for id in $kept; do
    case "$id" in
      kept | none) ;;
      *) sbx policy rm network --sandbox "$NAME" --id "$id" --force >/dev/null ;;
    esac
  done
  if [[ $PROXY_PORT != 0 && $kept != *kept ]]; then
    sbx policy allow network --sandbox "$NAME" "localhost:$PROXY_PORT" >/dev/null
  fi
}

check_policy() {
  echo "== 확인(모두 막혀야 한다)"
  for target in localhost:47810 localhost:47811 localhost:8780 example.com:443 \
    api.anthropic.com:443 deb.debian.org:80 192.168.0.1:80 10.0.0.1:80; do
    got=$({ sbx policy check network "$target" --sandbox "$NAME" || true; } | head -1)
    printf '  %-26s %s\n' "$target" "$got"
    if [[ $got != *Denied* ]]; then
      echo "예상과 다르다: $target 은(는) 막혀야 한다" >&2
      exit 1
    fi
  done
  if [[ $PROXY_PORT != 0 ]]; then
    echo "== 확인(출구 프록시 포트만 열려야 한다)"
    got=$({ sbx policy check network "localhost:$PROXY_PORT" --sandbox "$NAME" || true; } | head -1)
    printf '  %-26s %s\n' "localhost:$PROXY_PORT" "$got"
    if [[ $got != *Allowed* ]]; then
      echo "예상과 다르다: localhost:$PROXY_PORT 은(는) 열려야 한다" >&2
      exit 1
    fi
  fi
}

if [[ ${1:-} == --policy-only ]]; then
  sbx ls -q | grep -qx "$NAME" || { echo "샌드박스 $NAME 없음" >&2; exit 1; }
  apply_policy
  check_policy
  exit 0
fi

if sbx ls -q | grep -qx "$NAME"; then
  echo "샌드박스 $NAME 이미 있음. 다시 만들려면: sbx rm --force $NAME (안에 남길 것이 없어 지워도 된다)"
  echo "네트워크 규칙만 다시 걸려면: $0 --policy-only"
  exit 0
fi

sbx create --name "$NAME" --memory "$MEMORY" --skills off --static-mcp none --no-progress shell
apply_policy

if ! sbx exec "$NAME" sh -c 'command -v python3 >/dev/null'; then
  echo "== Python 설치(저장소는 설치하는 동안만 허용)"
  added=$(sbx policy allow network --sandbox "$NAME" deb.debian.org,dhi.io)
  ids=$(grep -oE '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' <<<"$added" || true)
  remove_mirrors() {
    for id in $ids; do
      sbx policy rm network --sandbox "$NAME" --id "$id" --force >/dev/null ||
        echo "경고: 저장소 허용 규칙 $id 를 지우지 못했다(sbx policy ls $NAME --wide)" >&2
    done
    ids=
  }
  trap remove_mirrors EXIT
  sbx exec "$NAME" sh -c "sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq >/tmp/apt.log 2>&1 &&
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends python3 >>/tmp/apt.log 2>&1 ||
    { tail -20 /tmp/apt.log; exit 1; }"
  remove_mirrors
  trap - EXIT
fi

echo "== 도구"
sbx exec "$NAME" sh -c 'python3 --version; command -v timeout pkill'
apply_policy
check_policy
echo "== 다 됐다. 설정 [scripts] enabled = true, sandbox = \"$NAME\""
