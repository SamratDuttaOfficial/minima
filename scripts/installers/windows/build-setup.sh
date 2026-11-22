#!/usr/bin/env bash
# Usage: build-setup.sh <minima.exe> <version> <arch> <dist-dir>   (Git Bash / MSYS on Windows)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if command -v cygpath >/dev/null 2>&1; then
  BIN="$(cygpath -w "$1")"; OUT="$(cygpath -w "$4")"
else
  BIN="$1"; OUT="$4"
fi
cmd.exe //c "$(cygpath -w "$HERE/build-setup.bat")" "$BIN" "$2" "$3" "$OUT"
