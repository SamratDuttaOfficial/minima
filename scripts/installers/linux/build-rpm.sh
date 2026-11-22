#!/usr/bin/env bash
# Usage: build-rpm.sh <minima-binary> <version> <arch> <dist-dir>
# Builds an .rpm if `rpmbuild` is installed (apt install rpm / dnf install rpm-build).
set -euo pipefail
BIN="$1"; VER="$2"; ARCH="$3"; DIST="$4"
command -v rpmbuild >/dev/null 2>&1 || { echo "  (.rpm skipped: rpmbuild not installed)"; exit 0; }
case "$ARCH" in x86_64) RPMARCH=x86_64 ;; aarch64) RPMARCH=aarch64 ;; armv7) RPMARCH=armv7hl ;; i686) RPMARCH=i686 ;; *) RPMARCH="$ARCH" ;; esac
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TOP="$(mktemp -d)"; trap 'rm -rf "$TOP"' EXIT
mkdir -p "$TOP"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
cp "$BIN" "$TOP/SOURCES/minima"
cp "$ROOT/README.md" "$TOP/SOURCES/README.md"
cp "$ROOT/assets/minima-256.png" "$TOP/SOURCES/minima-256.png"
cp "$ROOT/assets/minima-48.png" "$TOP/SOURCES/minima-48.png"
cp "$ROOT/assets/minima.svg" "$TOP/SOURCES/minima.svg"
cat > "$TOP/SOURCES/minima.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Minima
GenericName=Terminal Web Browser
Comment=Browse the web in your terminal
Exec=minima
Icon=minima
Terminal=true
Categories=Network;WebBrowser;ConsoleOnly;
Keywords=browser;terminal;text;
EOF
cat > "$TOP/SPECS/minima.spec" <<EOF
Name:           minima
Version:        $VER
Release:        1
Summary:        Terminal web browser that strips the web down to text
License:        MIT
BuildArch:      $RPMARCH
Recommends:     ffmpeg
Suggests:       yt-dlp
AutoReqProv:    no
%global debug_package %{nil}
%global __strip /bin/true

%description
Minima fetches web pages, removes ads, menus and banners with a compiled-in
content model, and shows links, buttons and forms in the terminal. Images and
video are rendered as ASCII / braille art.

%install
install -D -m 0755 %{_sourcedir}/minima %{buildroot}/usr/bin/minima
install -D -m 0644 %{_sourcedir}/minima.desktop %{buildroot}/usr/share/applications/minima.desktop
install -D -m 0644 %{_sourcedir}/README.md %{buildroot}/usr/share/doc/minima/README.md
install -D -m 0644 %{_sourcedir}/minima-256.png %{buildroot}/usr/share/icons/hicolor/256x256/apps/minima.png
install -D -m 0644 %{_sourcedir}/minima-48.png %{buildroot}/usr/share/icons/hicolor/48x48/apps/minima.png
install -D -m 0644 %{_sourcedir}/minima.svg %{buildroot}/usr/share/icons/hicolor/scalable/apps/minima.svg

%files
/usr/bin/minima
/usr/share/applications/minima.desktop
/usr/share/doc/minima/README.md
/usr/share/icons/hicolor/256x256/apps/minima.png
/usr/share/icons/hicolor/48x48/apps/minima.png
/usr/share/icons/hicolor/scalable/apps/minima.svg
EOF
rpmbuild --define "_topdir $TOP" --target "$RPMARCH" -bb "$TOP/SPECS/minima.spec" >/dev/null
cp "$TOP"/RPMS/*/minima-*.rpm "$DIST/"
echo "  wrote $(ls "$TOP"/RPMS/*/minima-*.rpm | xargs -n1 basename)"
