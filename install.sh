#!/usr/bin/env bash
# Lucia 받아서 설치하기(결정 0065): 빌드해 둔 릴리스를 내려받아 풀고, 처음이면 설정(setup.sh)으로, 이미 있으면
# 업데이트(install.sh)로 넘긴다. Rust나 Node.js 없이 된다.
#
#   curl -fsSL https://github.com/mooner1022/lucia/releases/latest/download/install.sh | bash
#
# 환경 변수:
#   LUCIA_VERSION=v0.1.0   받을 릴리스(기본은 가장 새것)
#   LUCIA_NO_SETUP=1       풀기만 하고 설정·업데이트로 넘어가지 않는다
#
# 풀리는 곳: ~/.local/lib/lucia/release (바이너리, 스크립트, plugin, 기본 캐릭터). 이 디렉터리는 자리를 지킨 채
# 내용만 바뀐다(샌드박스가 그 안의 plugin을 붙여 쓴다).
# 체크섬은 내려받다 깨지지 않았는지를 본다. 누가 만들었는지를 증명하지는 않는다(받는 곳이 GitHub 릴리스인 것이 그 근거다).
#
# 스크립트 전체를 함수로 감싼다: 내려받다 끊기면 반쯤 온 스크립트가 실행되지 않게.

lucia_install() {
  set -euo pipefail

  local slug="${LUCIA_REPO:-mooner1022/lucia}"
  local version="${LUCIA_VERSION:-latest}"
  # 시험용: 릴리스가 있는 디렉터리나 주소를 직접 준다(그 아래 lucia-x86_64-linux.tar.gz와 .sha256).
  local base="${LUCIA_RELEASE_BASE:-}"
  local asset="lucia-x86_64-linux.tar.gz"
  local lib="$HOME/.local/lib/lucia"
  local dest="$lib/release"

  say() { printf '%s\n' "$*"; }
  fail() {
    printf '멈춥니다: %s\n' "$*" >&2
    exit 1
  }

  [[ $EUID -ne 0 ]] || fail "root로 돌리지 않습니다. Lucia를 돌릴 사용자로 실행하세요."
  [[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] ||
    fail "x86-64 리눅스만 받을 수 있습니다(지금: $(uname -s) $(uname -m)). 다른 환경은 레포를 받아 빌드합니다(docs/DEPLOY.md)."
  local tool
  for tool in curl tar sha256sum python3; do
    command -v "$tool" >/dev/null 2>&1 || fail "$tool 이(가) 필요합니다(Ubuntu: sudo apt install $tool)."
  done

  local url
  if [[ -n $base ]]; then
    url="$base/$asset"
  elif [[ $version == latest ]]; then
    url="https://github.com/$slug/releases/latest/download/$asset"
  else
    url="https://github.com/$slug/releases/download/$version/$asset"
  fi

  local tmp
  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf -- '$tmp'" EXIT

  say "== 내려받기: $url"
  fetch() {
    if [[ $1 == /* ]]; then cp -- "$1" "$2"; else curl -fL --proto '=https' --tlsv1.2 --retry 3 -sS -o "$2" "$1"; fi
  }
  fetch "$url" "$tmp/$asset" || fail "내려받지 못했습니다. 릴리스가 있는지 확인하세요: https://github.com/$slug/releases"
  fetch "$url.sha256" "$tmp/$asset.sha256" || fail "체크섬 파일을 받지 못했습니다."
  (cd "$tmp" && sha256sum -c "$asset.sha256" >/dev/null) || fail "체크섬이 맞지 않습니다. 다시 받아 보세요."

  mkdir -p "$tmp/unpack"
  tar -xzf "$tmp/$asset" -C "$tmp/unpack"
  local src="$tmp/unpack/lucia-x86_64-linux"
  [[ -x $src/luciad && -f $src/VERSION && -f $src/deploy/setup.sh ]] || fail "묶음의 모양이 다릅니다."
  local new old=""
  new="$(cat "$src/VERSION")"
  [[ -f $dest/VERSION ]] && old="$(cat "$dest/VERSION")"

  # 자리를 지킨 채 내용만 맞춘다: 새 파일을 덮어쓰고, 묶음에 없는 옛 파일을 지운다.
  mkdir -p "$dest"
  python3 -I - "$src" "$dest" <<'PY'
import os, shutil, sys
src, dst = sys.argv[1:3]
keep = set()
for root, dirs, files in os.walk(src):
    rel = os.path.relpath(root, src)
    target = dst if rel == "." else os.path.join(dst, rel)
    os.makedirs(target, exist_ok=True)
    keep.add(os.path.normpath(target))
    for name in files:
        out = os.path.join(target, name)
        part = out + ".part"
        shutil.copy2(os.path.join(root, name), part)
        os.replace(part, out)
        keep.add(os.path.normpath(out))
for root, dirs, files in os.walk(dst, topdown=False):
    for name in files:
        path = os.path.normpath(os.path.join(root, name))
        if path not in keep:
            os.remove(path)
    for name in dirs:
        path = os.path.normpath(os.path.join(root, name))
        if path not in keep and not os.listdir(path):
            os.rmdir(path)
PY
  if [[ -n $old && $old != "$new" ]]; then say "== 풀었습니다: $dest ($old → $new)"; else say "== 풀었습니다: $dest ($new)"; fi

  if [[ ${LUCIA_NO_SETUP:-0} == 1 ]]; then
    say "다음: $dest/deploy/setup.sh"
    return 0
  fi

  # 이미 설치돼 있으면 업데이트(저장소 백업 → 바이너리 바꾸기 → 재시작), 처음이면 설정.
  if [[ -x $lib/luciad && -f $HOME/.config/systemd/user/luciad.service ]]; then
    say "== 업데이트"
    "$dest/deploy/install.sh"
    return 0
  fi
  # curl | bash로 돌리면 표준 입력이 스크립트라, 답은 터미널에서 읽는다.
  if [[ -t 0 ]]; then
    "$dest/deploy/setup.sh"
  elif [[ -r /dev/tty ]] && (: </dev/tty) 2>/dev/null; then
    "$dest/deploy/setup.sh" </dev/tty
  else
    say "터미널이 없어 여기까지만 했습니다. 이어서 직접 돌려 주세요:"
    say "  $dest/deploy/setup.sh"
  fi
}

lucia_install "$@"
