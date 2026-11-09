#!/usr/bin/env bash
# Integration test: Xvfb + FreeRDP shadow server + the freerdp wrapper's ignored test.
# Usage: SHADOW_CLI=/path/to/freerdp-shadow-cli scripts/it-shadow.sh
set -euo pipefail
cd "$(dirname "$0")/.."
: "${SHADOW_CLI:?set SHADOW_CLI to a freerdp-shadow-cli binary (build vendor/freerdp with -DWITH_SHADOW=ON -DWITH_X11=ON)}"
PORT="${RUSTRDP_SHADOW_PORT:-33890}"
DISPLAY_NUM=97

Xvfb ":$DISPLAY_NUM" -screen 0 1280x800x24 >/dev/null 2>&1 &
XVFB=$!
trap 'kill ${SHADOW:-0} $XVFB 2>/dev/null || true' EXIT
sleep 1
# TLS only, no authentication: the shadow server has no SAM file here, so NLA would always fail.
DISPLAY=":$DISPLAY_NUM" "$SHADOW_CLI" "/port:$PORT" /sec:tls -auth >/dev/null 2>&1 &
SHADOW=$!
sleep 2
# The shadow server only sends frames when the screen changes: animate the root window.
( i=0; while :; do DISPLAY=":$DISPLAY_NUM" xsetroot -solid "#$(printf '%02x%02x%02x' $((i%256)) 80 160)"; i=$((i+23)); sleep 0.3; done ) >/dev/null 2>&1 &
ANIM=$!
trap 'kill ${ANIM:-0} ${SHADOW:-0} $XVFB 2>/dev/null || true' EXIT
RUSTRDP_SHADOW_PORT=$PORT cargo test -p freerdp --test shadow -- --ignored --nocapture
