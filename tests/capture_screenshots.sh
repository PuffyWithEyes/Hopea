#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/.."
: "${HOPEA_XVFB:=Xvfb}"
command -v "$HOPEA_XVFB" >/dev/null
capture_root=$(mktemp -d /tmp/hopea-screenshots.XXXXXXXX)
display_pid=
cleanup() {
    if [[ -n "$display_pid" ]]; then
        kill "$display_pid" 2>/dev/null || true
        wait "$display_pid" 2>/dev/null || true
    fi
    rm -rf "$capture_root"
}
trap cleanup EXIT
"$HOPEA_XVFB" -displayfd 3 -screen 0 1440x1000x24 -nolisten tcp \
    3> "$capture_root/display" > "$capture_root/display.log" 2>&1 &
display_pid=$!
for ((attempt=0; attempt<100; attempt++)); do
    [[ -s "$capture_root/display" ]] && break
    sleep 0.1
done
if [[ ! -s "$capture_root/display" ]]; then
    cat "$capture_root/display.log" >&2
    exit 1
fi
export DISPLAY=":$(cat "$capture_root/display")" GDK_BACKEND=x11 GSK_RENDERER=cairo
export XDG_CACHE_HOME="$capture_root/cache" XDG_CONFIG_HOME="$capture_root/config" XDG_DATA_HOME="$capture_root/data"
export GSETTINGS_BACKEND=memory GSETTINGS_SCHEMA_DIR="$PWD/_builddir/data"
export HOPEA_IMPORT_PYTHON=/nonexistent/hopea-test-python LANGUAGE=en LC_ALL=C.UTF-8
export HOPEA_README_SCREENSHOTS="$PWD/docs/screenshots"
cargo test --target-dir _builddir/src documentation_screenshots_and_import_layout -- --ignored --test-threads=1 --nocapture
