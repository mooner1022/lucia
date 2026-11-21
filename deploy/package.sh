#!/usr/bin/env bash
# 릴리스 묶음 만들기(결정 0065): 빌드해 둔 것을 받아 쓰는 설치(install.sh)가 내려받는 tar.gz와 체크섬을 만든다.
#
#   deploy/package.sh            # dist-release/lucia-x86_64-linux.tar.gz(.sha256)
#   deploy/package.sh v0.1.0     # 태그를 주면 Cargo.toml의 버전과 같은지 확인한다(릴리스 워크플로가 이렇게 부른다)
#
# 묶음에 드는 것: luciad(관리 화면 포함), VERSION, deploy/(설치·샌드박스 스크립트, 유닛, 설정 예시), plugin/,
# character/(기본 캐릭터), README.md, docs/DEPLOY.md.
# - plugin/·character/·스크립트는 **커밋된 것**(git archive HEAD)만 넣는다. 작업 트리의 고친 파일(이 서버에서 쓰는
#   캐릭터 등)은 들어가지 않는다. 바이너리는 작업 트리로 빌드하므로, 추적하는 파일이 고쳐져 있으면 멈춘다
#   (시험으로 만들 때만 LUCIA_PACKAGE_ALLOW_DIRTY=1).
# - 올리는 묶음은 릴리스 워크플로(.github/workflows/release.yml)가 만든 것만 쓴다. 손으로 만든 것은 그 기계의 glibc에
#   묶이므로 올리지 않는다.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${LUCIA_PACKAGE_OUT:-$REPO/dist-release}"
ASSET="lucia-x86_64-linux"
TAG="${1:-}"

cd "$REPO"
[[ "$(uname -m)" == x86_64 ]] || { echo "x86_64에서만 만든다(지금: $(uname -m))" >&2; exit 2; }
VERSION="$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1)"
[[ -n $VERSION ]] || { echo "Cargo.toml에서 버전을 읽지 못했다" >&2; exit 2; }
if [[ -n $TAG && $TAG != "v$VERSION" ]]; then
  echo "태그($TAG)가 Cargo.toml의 버전(v$VERSION)과 다르다. 버전을 올리고 다시 태그한다." >&2
  exit 2
fi
if [[ -n "$(git status --porcelain --untracked-files=no)" && ${LUCIA_PACKAGE_ALLOW_DIRTY:-0} != 1 ]]; then
  echo "추적하는 파일이 고쳐져 있다. 커밋한 뒤 만들거나, 시험이면 LUCIA_PACKAGE_ALLOW_DIRTY=1." >&2
  git status --short --untracked-files=no >&2
  exit 2
fi

deploy/build-ui.sh
echo "== 릴리스 빌드"
cargo build --release -p luciad
target/release/luciad ui-check
[[ "$(target/release/luciad --version)" == "luciad $VERSION" ]] || { echo "바이너리의 버전이 $VERSION 이 아니다" >&2; exit 2; }

STAGE="$(mktemp -d)"
trap 'rm -rf -- "$STAGE"' EXIT
ROOT="$STAGE/$ASSET"
mkdir -p "$ROOT"
# 커밋된 파일만 꺼낸다.
git archive HEAD deploy plugin character README.md docs/DEPLOY.md | tar -x -C "$ROOT"
# 받아 쓰는 설치에는 쓸 일이 없는 것(빌드·묶음 스크립트).
rm -f -- "$ROOT/deploy/package.sh" "$ROOT/deploy/build-ui.sh"
install -m 0755 target/release/luciad "$ROOT/luciad"
printf '%s\n' "$VERSION" >"$ROOT/VERSION"

mkdir -p "$OUT"
# 같은 내용이면 같은 묶음이 나오게 순서·소유자·시각을 고정한다(시각은 그 커밋의 것).
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$(git log -1 --format=%ct)" \
  -czf "$OUT/$ASSET.tar.gz" -C "$STAGE" "$ASSET"
(cd "$OUT" && sha256sum "$ASSET.tar.gz" >"$ASSET.tar.gz.sha256")
cp -f -- "$REPO/install.sh" "$OUT/install.sh"
echo "== 묶음: $OUT/$ASSET.tar.gz ($(du -h "$OUT/$ASSET.tar.gz" | cut -f1), 버전 $VERSION)"
cat "$OUT/$ASSET.tar.gz.sha256"
