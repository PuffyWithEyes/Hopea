#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
[[ ${MSYSTEM:-} == UCRT64 ]] || { echo 'Run this in an MSYS2 UCRT64 shell' >&2; exit 2; }

packages=(
    zip unzip curl
    mingw-w64-ucrt-x86_64-{toolchain,rust,meson,ninja,pkgconf,cmake,nasm}
    mingw-w64-ucrt-x86_64-{gtk4,libadwaita,libsoup3,blueprint-compiler}
    mingw-w64-ucrt-x86_64-{desktop-file-utils,appstream,gettext}
    mingw-w64-ucrt-x86_64-gstreamer
    mingw-w64-ucrt-x86_64-gst-plugins-{base,good,bad,ugly}
    mingw-w64-ucrt-x86_64-gst-libav
    mingw-w64-ucrt-x86_64-{python-pip,ffmpeg}
)

# Retry downloads only: cached packages survive a mirror timeout. Leave
# signature verification and transfer timeouts enabled, and never retry a
# failed installation transaction or discard the package database lock.
for attempt in 1 2 3; do
    if pacman -Sw --needed --noconfirm "${packages[@]}"; then
        exec pacman -S --needed --noconfirm "${packages[@]}"
    else
        status=$?
    fi
    if (( attempt == 3 )); then
        echo 'MSYS2 dependency download failed after 3 attempts.' >&2
        exit "$status"
    fi
    echo "MSYS2 download failed; retrying in $((attempt * 10)) seconds ($attempt/3)." >&2
    sleep "$((attempt * 10))"
done
