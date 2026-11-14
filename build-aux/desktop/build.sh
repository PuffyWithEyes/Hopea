#!/bin/sh
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
# Native source install; uses the target OS's GTK/GStreamer packages.
set -eu
cd "$(dirname "$0")/../.."
case "$(uname -s)" in
  Linux|FreeBSD|OpenBSD|NetBSD|DragonFly|Darwin) ;;
  *) echo 'This recipe requires a Unix desktop supported by GTK4.' >&2; exit 1 ;;
esac
for tool in meson ninja cargo pkg-config blueprint-compiler python3 ffmpeg; do
  command -v "$tool" >/dev/null || { echo "Missing build dependency: $tool (see README.md)" >&2; exit 1; }
done
build_prefix=${HOPEA_PREFIX:-"$HOME/.local"}
build_dir=${HOPEA_BUILD_DIR:-_native-build}
case "$build_prefix" in /*) ;; *) echo 'HOPEA_PREFIX must be absolute.' >&2; exit 1;; esac
if [ -f "$build_dir/build.ninja" ]; then
  meson configure "$build_dir" --prefix="$build_prefix" -Dprofile=default
else
  meson setup "$build_dir" . --prefix="$build_prefix" -Dprofile=default
fi
meson compile -C "$build_dir"
meson compile -C "$build_dir" # Bundle freshly compiled Blueprint resources.
meson install -C "$build_dir"
python3 -m venv "$build_prefix/lib/hopea-import"
"$build_prefix/lib/hopea-import/bin/python3" -m pip install --disable-pip-version-check -r build-aux/import-requirements.txt
"$build_prefix/lib/hopea-import/bin/python3" -I -c 'import vk_api, bs4, yandex_music, ytmusicapi, yt_dlp, Cryptodome, spotipy'
echo "Installed $build_prefix/bin/hopea; native GTK/GStreamer packages remain required."
