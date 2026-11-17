#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026  Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later

# Builds a portable Windows version of the player: a folder, and a zip of
# it, targeting 64-bit Windows 10 or later without external runtime installs.
# Older Windows releases cannot run the shipped Rust/UCRT64/Python stack.
# Run it on Windows, in an MSYS2 "UCRT64" shell
# (https://www.msys2.org), from anywhere in the source tree:
#
#   bash build-aux/windows/build.sh
#
# The result is _windows/Hopea (start bin/hopea.exe) and
# dist/hopea-windows.zip.
#
# The native tray includes player controls. Sharing and the background-play
# setting keep the application alive after closing the window. There is no
# MPRIS on Windows (media keys go through the window only).

set -euo pipefail
# Blueprint reads UTF-8 sources; Windows otherwise defaults to its ANSI locale.
export PYTHONUTF8=1

if [[ "${MSYSTEM:-}" != "UCRT64" ]]; then
    echo "Run this in an MSYS2 UCRT64 shell" >&2
    exit 1
fi

cd "$(dirname "$0")/../.."
root=$PWD
prefix=$root/_windows/Hopea
sys=/ucrt64

bash build-aux/windows/install-deps.sh

python3 tests/test_cargo_build.py

# Retain the Cargo and Meson caches for incremental builds.
rm -rf "$prefix"
meson setup "$root/_windows/build" "$root" \
    -Dprofile=default --buildtype=release --prefix="$prefix"
meson compile -C "$root/_windows/build"
meson install -C "$root/_windows/build"

# Copies the MSYS2 DLLs that the given programs and libraries load
copy_dlls() {
    ldd "$@" | awk -v sys="$sys" 'index($3, sys) == 1 { print $3 }' | sort -u |
        while read -r dll; do cp -u "$dll" "$prefix/bin/"; done
}

copy_dlls "$prefix/bin/hopea.exe" "$prefix/bin/hopea-spotify.exe"
cp "$sys/bin/ffmpeg.exe" "$sys/bin/ffprobe.exe" "$prefix/bin/"
copy_dlls "$prefix/bin/ffmpeg.exe" "$prefix/bin/ffprobe.exe"

# Official isolated CPython, with pinned importer wheels. No system Python,
# registry entry or virtual-environment paths are needed on the recipient PC.
mkdir -p "$root/_windows/downloads" "$prefix/lib/hopea-import"
python_zip="$root/_windows/downloads/python-3.14.10-embed-amd64.zip"
if [[ ! -f "$python_zip" ]]; then
    curl --fail --location --proto '=https' --tlsv1.2 \
        https://www.python.org/ftp/python/3.14.10/python-3.14.10-embed-amd64.zip \
        -o "$python_zip"
fi
echo "90905e64a840557d8c1bc4f5b74db3d7ee85e3ffb217611cac1c7813acea4159  $python_zip" | sha256sum -c -
unzip -q "$python_zip" -d "$prefix/lib/hopea-import"
printf 'python314.zip\n.\nsite-packages\n' > "$prefix/lib/hopea-import/python314._pth"
python -m pip install --disable-pip-version-check --only-binary=:all: \
    --platform win_amd64 --python-version 3.14 --implementation cp --abi cp314 \
    --target "$prefix/lib/hopea-import/site-packages" -r build-aux/import-requirements.txt
"$prefix/lib/hopea-import/python.exe" -I -c \
    'import vk_api, bs4, yandex_music, ytmusicapi, yt_dlp, Cryptodome, spotipy, ssl, sqlite3'

# GStreamer finds its plugins and scanner relative to its own DLL
mkdir -p "$prefix/lib/gstreamer-1.0" "$prefix/libexec/gstreamer-1.0"
cp "$sys"/lib/gstreamer-1.0/*.dll "$prefix/lib/gstreamer-1.0/"
cp "$sys"/libexec/gstreamer-1.0/gst-plugin-scanner.exe "$prefix/libexec/gstreamer-1.0/"
copy_dlls "$prefix"/lib/gstreamer-1.0/*.dll "$prefix"/libexec/gstreamer-1.0/*.exe

# GTK's settings schemas, icons and image loaders, next to the app's own
mkdir -p "$prefix/share/glib-2.0/schemas" "$prefix/share/icons" "$prefix/lib"
cp "$sys"/share/glib-2.0/schemas/*.xml "$prefix/share/glib-2.0/schemas/"
glib-compile-schemas "$prefix/share/glib-2.0/schemas"
cp -r "$sys"/share/icons/{Adwaita,hicolor} "$prefix/share/icons/"
cp -r "$sys"/lib/gdk-pixbuf-2.0 "$prefix/lib/"
copy_dlls "$prefix"/lib/gdk-pixbuf-2.0/2.10.0/loaders/*.dll
cp "$sys/bin/gdk-pixbuf-query-loaders.exe" "$prefix/bin/"
copy_dlls "$prefix/bin/gdk-pixbuf-query-loaders.exe"

# Nothing from the build itself is needed to run it
rm -rf "$prefix/include" "$prefix/lib/pkgconfig"

# License notices travel with both the portable ZIP and the offline installer.
cp -r LICENSES "$prefix/"
cp ADDITIONAL-TERMS.md "$prefix/"
mkdir -p "$prefix/share/licenses"
cp -r "$sys/share/licenses/." "$prefix/share/licenses/"

mkdir -p "$root/dist"
rm -f "$root/dist/hopea-windows.zip"
(cd "$root/_windows" && zip -qr "$root/dist/hopea-windows.zip" Hopea)
echo "Built _windows/Hopea and dist/hopea-windows.zip"
