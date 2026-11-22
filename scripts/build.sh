#!/usr/bin/env bash
# Build Minima and package distributables into ./dist
#
#   scripts/build.sh                    release build for this machine + all installers it can make
#   scripts/build.sh --static           (Linux) fully static musl build: runs on very old distros
#   scripts/build.sh --target TRIPLE    cross-build (see docs/BUILDING.md); repeatable
#   scripts/build.sh --all              every target in scripts/targets.txt that can be built here
#   scripts/build.sh --no-installers    only the binary + portable archive
#   scripts/build.sh --debug | --clean | --help
#
# Produces in dist/:
#   minima-<ver>-<os>-<arch>.tar.gz|zip      portable archive
#   minima-<ver>-<os>-<arch>-installer-cli   minimal CLI installer (every OS, no GUI)
#   Linux : minima_<ver>_<arch>.deb  (+ .rpm when rpmbuild is installed)
#   macOS : Minima-<ver>-<arch>.pkg and .dmg (graphical installers)
#   Windows (from Git Bash/MSYS): setup .exe when Inno Setup is installed
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
DIST="$ROOT/dist"
PROFILE=release
INSTALLERS=1
STATIC=0
ALL=0
TARGETS=()

usage() { sed -n '2,19p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
  case "$1" in
    --static) STATIC=1 ;;
    --target) shift; TARGETS+=("$1") ;;
    --all) ALL=1 ;;
    --no-installers) INSTALLERS=0 ;;
    --debug) PROFILE=debug ;;
    --clean) rm -rf "$DIST" target; echo "cleaned"; exit 0 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage; exit 2 ;;
  esac
  shift
done

command -v cargo >/dev/null 2>&1 || { echo "error: Rust is not installed. Get it from https://rustup.rs" >&2; exit 1; }

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
UNAME_S="$(uname -s)"
case "$UNAME_S" in
  Linux*) HOST_OS=linux ;;
  Darwin*) HOST_OS=macos ;;
  MINGW*|MSYS*|CYGWIN*) HOST_OS=windows ;;
  *) HOST_OS=linux ;;
esac

if [ "$ALL" = 1 ]; then
  while read -r t; do [ -n "$t" ] && [ "${t#\#}" = "$t" ] && TARGETS+=("$t"); done < "$ROOT/scripts/targets.txt"
fi
if [ ${#TARGETS[@]} -eq 0 ]; then
  if [ "$STATIC" = 1 ] && [ "$HOST_OS" = linux ]; then
    case "$(uname -m)" in aarch64|arm64) TARGETS=(aarch64-unknown-linux-musl) ;; *) TARGETS=(x86_64-unknown-linux-musl) ;; esac
  else
    TARGETS=("$HOST_TRIPLE")
  fi
fi

os_of()   { case "$1" in *windows*) echo windows ;; *apple*) echo macos ;; *linux*|*android*) echo linux ;; *) echo "$HOST_OS" ;; esac; }
arch_of() { case "$1" in x86_64*) echo x86_64 ;; aarch64*|arm64*) echo aarch64 ;; armv7*) echo armv7 ;; i686*|i586*) echo i686 ;; *) echo "${1%%-*}" ;; esac; }

mkdir -p "$DIST"
echo "==> Minima $VERSION  (host: $HOST_TRIPLE)"
CARGO_PROFILE_FLAG=(); [ "$PROFILE" = release ] && CARGO_PROFILE_FLAG=(--release)
BUILT=0
# the packager runs on the host
cargo build --release -q -p xtask
XTASK="$ROOT/target/release/xtask"; [ -x "$XTASK" ] || XTASK="$ROOT/target/release/xtask.exe"

for T in "${TARGETS[@]}"; do
  OS="$(os_of "$T")"; ARCH="$(arch_of "$T")"
  EXT=""; [ "$OS" = windows ] && EXT=".exe"
  echo
  echo "==> [$T] building ($OS/$ARCH)"
  if [ "$T" != "$HOST_TRIPLE" ]; then
    rustup target add "$T" >/dev/null 2>&1 || true
  fi
  BUILD=(cargo build "${CARGO_PROFILE_FLAG[@]}" --target "$T")
  if [ "$T" != "$HOST_TRIPLE" ] && command -v cargo-zigbuild >/dev/null 2>&1; then
    BUILD=(cargo zigbuild "${CARGO_PROFILE_FLAG[@]}" --target "$T")
  fi
  if ! "${BUILD[@]}" -p minima; then
    echo "!! skipped $T: build failed (missing linker/C toolchain? see docs/BUILDING.md)" >&2
    continue
  fi
  BIN="$ROOT/target/$T/$PROFILE/minima$EXT"
  INSTALLER_ARGS=()
  if [ "$INSTALLERS" = 1 ]; then
    if MINIMA_PAYLOAD="$BIN" "${BUILD[@]}" -p minima-installer; then
      INSTALLER_ARGS=(--installer "$ROOT/target/$T/$PROFILE/minima-installer$EXT")
    else
      echo "!! CLI installer failed for $T" >&2
    fi
  fi
  "$XTASK" dist --bin "$BIN" --os "$OS" --arch "$ARCH" --version "$VERSION" --out "$DIST" "${INSTALLER_ARGS[@]}"

  if [ "$INSTALLERS" = 1 ] && [ "$OS" = "$HOST_OS" ]; then
    case "$OS" in
      macos)   "$ROOT/scripts/installers/macos/build-macos.sh" "$BIN" "$VERSION" "$ARCH" "$DIST" || echo "!! macOS installers failed" >&2 ;;
      linux)   "$ROOT/scripts/installers/linux/build-rpm.sh"   "$BIN" "$VERSION" "$ARCH" "$DIST" || echo "(rpm skipped)" ;;
      windows) "$ROOT/scripts/installers/windows/build-setup.sh" "$BIN" "$VERSION" "$ARCH" "$DIST" || echo "(Inno Setup installer skipped)" ;;
    esac
  fi
  BUILT=$((BUILT+1))
done

"$XTASK" sums "$DIST" >/dev/null
echo
echo "==> done: $BUILT target(s) built. Contents of dist/:"
ls -lh "$DIST" | sed 's/^/    /'
