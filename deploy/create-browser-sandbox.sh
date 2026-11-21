#!/usr/bin/env bash
# 브라우저 샌드박스를 만든다(결정 0006·0008, PoC P8): 로그인 프로필을 유지하는 화면 있는 Chromium.
#
# - shell 킷, 작업 공간 없음.
# - 네트워크: 이 샌드박스는 **데몬의 브라우저 프록시 포트 하나만** 허용한다(결정 0008). 어디로 갈지는 데몬이 정한다
#   (차단 목록 방식: 사설·내부 주소와 차단 목록만 막고 나머지는 연다). sbx에 `**`를 열지 않는다:
#   sbx는 이름으로 풀린 사설 IP에 CIDR 거부 규칙을 적용하지 않는다(PoC P8b). 그래서 판단은 데몬이 이름을 한 번 풀어
#   그 주소로 연결하며 한다.
# - Chromium 정책: 비밀번호 저장·자동 완성·동기화·내려받기를 끈다.
# - CDP·화면 포트는 열지 않는다. 데몬이 붙잡아 두는 sbx exec 세션의 표준 입출력으로 CDP를 잇는다.
# - 브라우저는 데몬이 쓸 때 띄운다(여기서는 설치와 정책만).
#
# 사용: deploy/create-browser-sandbox.sh
#   LUCIA_BROWSER_SANDBOX(기본 lucia-browser), LUCIA_BROWSER_MEMORY(기본 2g),
#   LUCIA_BROWSER_PROXY_PORT(기본 47811: 설정 browser.proxy_bind의 포트와 맞춘다)
#   이미 있는 샌드박스의 네트워크 규칙만 다시 걸려면: deploy/create-browser-sandbox.sh --policy-only
set -euo pipefail

NAME=${LUCIA_BROWSER_SANDBOX:-lucia-browser}
MEMORY=${LUCIA_BROWSER_MEMORY:-2g}
PROXY_PORT=${LUCIA_BROWSER_PROXY_PORT:-47811}
# 혹시 넓은 허용이 생겨도 열리지 않게 이름으로도 막는다(데몬 포트와 계정 쪽).
DENY=(api.anthropic.com '*.anthropic.com' claude.com '*.claude.com' claude.ai '*.claude.ai'
  bridge.claudeusercontent.com '*.claudeusercontent.com' localhost:47810 localhost:8780)
PACKAGES="chromium xvfb socat curl fonts-noto-cjk"

case "$NAME" in
  lucia | lucia-poc)
    echo "상주·PoC 샌드박스 이름($NAME)은 쓸 수 없다" >&2
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
  # 이 샌드박스에 걸린 허용 규칙은 프록시 포트 말고 모두 지운다(예전 허용 목록 방식의 사이트 허용,
  # 차단 목록 실험의 `**` 등). 지울 때는 규칙 ID(RULE_ID)를 쓴다.
  for id in $(sbx policy ls "$NAME" --json | python3 -I -c '
import json, sys
name, keep = sys.argv[1], sys.argv[2]
for r in json.load(sys.stdin)["rules"]:
    if (r.get("scope") == "sandbox:" + name and r.get("resource_type") == "network"
            and r.get("decision") == "allow" and r.get("editable") and r.get("resources") != [keep]):
        print(r["id"])' "$NAME" "localhost:$PROXY_PORT"); do
    sbx policy rm network --sandbox "$NAME" --id "$id" --force >/dev/null
  done
  sbx policy allow network --sandbox "$NAME" "localhost:$PROXY_PORT" >/dev/null 2>&1 || true
}

check_policy() {
  echo "== 확인"
  for check in "localhost:$PROXY_PORT Allowed" "example.com:443 Denied" "example.org:443 Denied" \
    "api.anthropic.com:443 Denied" "localhost:47810 Denied" "localhost:8780 Denied" \
    "192.168.0.1:80 Denied" "10.0.0.1:80 Denied"; do
    set -- $check
    got=$({ sbx policy check network "$1" --sandbox "$NAME" || true; } | head -1)
    printf '  %-30s %s\n' "$1" "$got"
    if [[ $got != *"$2"* ]]; then
      echo "예상과 다르다: $1 은(는) $2 여야 한다" >&2
      exit 1
    fi
  done
  # 상주 샌드박스(에이전트)는 브라우저 프록시에 닿으면 안 된다(닿으면 인터넷을 얻는다, 결정 0003).
  got=$({ sbx policy check network "localhost:$PROXY_PORT" --sandbox lucia || true; } | head -1)
  printf '  %-30s %s\n' "lucia → localhost:$PROXY_PORT" "$got"
  if [[ $got != *Denied* ]]; then
    echo "상주 샌드박스 lucia가 브라우저 프록시에 닿는다" >&2
    exit 1
  fi
}

if [[ ${1:-} == --policy-only ]]; then
  sbx ls -q | grep -qx "$NAME" || { echo "샌드박스 $NAME 없음" >&2; exit 1; }
  apply_policy
  check_policy
  exit 0
fi

if sbx ls -q | grep -qx "$NAME"; then
  echo "샌드박스 $NAME 이미 있음. 다시 만들려면: sbx rm --force $NAME (브라우저 프로필과 로그인이 지워진다)"
  echo "네트워크 규칙만 다시 걸려면: $0 --policy-only"
  exit 0
fi

sbx create --name "$NAME" --memory "$MEMORY" --skills off --static-mcp none --no-progress shell
apply_policy

echo "== 설치(저장소는 설치하는 동안만 허용)"
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
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends $PACKAGES >>/tmp/apt.log 2>&1 ||
  { tail -20 /tmp/apt.log; exit 1; }"
remove_mirrors
trap - EXIT

echo "== Chromium 정책"
sbx exec -i "$NAME" sh -c 'sudo mkdir -p /etc/chromium/policies/managed &&
  sudo tee /etc/chromium/policies/managed/lucia.json >/dev/null' <<'JSON'
{
  "PasswordManagerEnabled": false,
  "AutofillAddressEnabled": false,
  "AutofillCreditCardEnabled": false,
  "BrowserSignin": 0,
  "SyncDisabled": true,
  "MetricsReportingEnabled": false,
  "SearchSuggestEnabled": false,
  "DownloadRestrictions": 3,
  "DefaultNotificationsSetting": 2,
  "DefaultGeolocationSetting": 2
}
JSON

check_policy
sbx exec "$NAME" sh -c 'chromium --version && command -v socat Xvfb curl >/dev/null && test -f /etc/chromium/policies/managed/lucia.json'
echo "완료. 설정에서 [browser] enabled = true, sandbox = \"$NAME\" 로 켠다."
