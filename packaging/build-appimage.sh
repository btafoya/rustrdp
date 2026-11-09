#!/usr/bin/env bash
# Build a test AppImage from a release build. Needs linuxdeploy (with the appimage plugin) on PATH.
# Bundles the vendored FreeRDP, GTK4 and libadwaita from the build host, so the result runs on hosts
# with the same or newer glibc. Test build only: the FFmpeg that FreeRDP links is the distro (GPL) one,
# see README "Known gaps" before distributing.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

cargo build --release -p rustrdp
bin=target/release/rustrdp
fr=$(dirname "$(find target/release/build -path '*/out/lib/libfreerdp3.so.3' | head -1)")

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
export LD_LIBRARY_PATH="$here/usr/lib:$LD_LIBRARY_PATH"
export XDG_DATA_DIRS="$here/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export GSETTINGS_SCHEMA_DIR="$here/usr/share/glib-2.0/schemas"
exec "$here/usr/bin/rustrdp" "$@"
EOF
chmod +x target/appimage/AppRun

cd target/appimage
LD_LIBRARY_PATH="$root/$fr" APPIMAGE_EXTRACT_AND_RUN=1 VERSION=$(cargo pkgid -p rustrdp --manifest-path "$root/Cargo.toml" | sed 's/.*[#@]//') \
  linuxdeploy --appdir AppDir --executable "$root/$bin" \
    --library "$root/$fr/libfreerdp3.so.3" --library "$root/$fr/libfreerdp-client3.so.3" --library "$root/$fr/libwinpr3.so.3" \
    --desktop-file AppDir/com.briantafoya.rustrdp.desktop --icon-file AppDir/com.briantafoya.rustrdp.svg \
    --custom-apprun AppRun --output appimage
ls -l "$root"/target/appimage/*.AppImage
