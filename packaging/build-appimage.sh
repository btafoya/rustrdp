#!/usr/bin/env bash
# Build a test AppImage from a release build. Needs linuxdeploy (with the appimage plugin) on PATH.
# Bundles the vendored FreeRDP, GTK4 and libadwaita from the build host, so the result runs on hosts
# with the same or newer glibc. Test build only: the FFmpeg that FreeRDP links is the distro (GPL) one,
# see README "Known gaps" before distributing.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# Every build bumps the patch version in the workspace Cargo.toml so builds never share a filename.
# NO_BUMP=1 rebuilds the current version (e.g. after a failed run).
if [ -z "${NO_BUMP:-}" ]; then
  cur=$(sed -n 's/^version = "\([0-9]*\.[0-9]*\.[0-9]*\)"$/\1/p' Cargo.toml | head -1)
  [ -n "$cur" ] || { echo "no version in Cargo.toml" >&2; exit 1; }
  next="${cur%.*}.$((${cur##*.} + 1))"
  sed -i "0,/^version = \"$cur\"\$/s//version = \"$next\"/" Cargo.toml
  echo "version $cur -> $next"
fi

# Bundle the FreeRDP this build was linked against: target/release/build keeps one out dir per past
# configuration, so `find | head -1` can pick a stale one. Cargo reports the right dir.
fr=$(cargo build --release -p rustrdp --message-format=json |
  jq -r 'select(.reason == "build-script-executed" and (.package_id | contains("freerdp-sys"))) | .out_dir' |
  tail -1)/lib
[ -f "$fr/libfreerdp3.so.3" ] || { echo "libfreerdp3.so.3 not found in $fr" >&2; exit 1; }
bin=target/release/rustrdp

app=target/appimage/AppDir
rm -rf "$app"
mkdir -p "$app/usr/share/glib-2.0/schemas" "$app/usr/share/icons"
cp -a /usr/share/icons/Adwaita "$app/usr/share/icons/"
cp /usr/share/glib-2.0/schemas/gschemas.compiled "$app/usr/share/glib-2.0/schemas/"
cp data/com.briantafoya.rustrdp.desktop "$app/"
sed -i 's/^Icon=.*/Icon=com.briantafoya.rustrdp/' "$app/com.briantafoya.rustrdp.desktop"
cp data/com.briantafoya.rustrdp.svg "$app/"

cat >target/appimage/AppRun <<'EOF'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
# No trailing colon when unset: an empty entry would put the current directory on the library path.
export LD_LIBRARY_PATH="$here/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export XDG_DATA_DIRS="$here/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export GSETTINGS_SCHEMA_DIR="$here/usr/share/glib-2.0/schemas"
exec "$here/usr/bin/rustrdp" "$@"
EOF
chmod +x target/appimage/AppRun

cd target/appimage
LD_LIBRARY_PATH="$fr" APPIMAGE_EXTRACT_AND_RUN=1 VERSION=$(cargo pkgid -p rustrdp --manifest-path "$root/Cargo.toml" | sed 's/.*[#@]//') \
  linuxdeploy --appdir AppDir --executable "$root/$bin" \
    --library "$fr/libfreerdp3.so.3" --library "$fr/libfreerdp-client3.so.3" --library "$fr/libwinpr3.so.3" \
    --desktop-file AppDir/com.briantafoya.rustrdp.desktop --icon-file AppDir/com.briantafoya.rustrdp.svg \
    --custom-apprun AppRun --output appimage
ls -l "$root"/target/appimage/*.AppImage
