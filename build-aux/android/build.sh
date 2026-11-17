#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
arch=${HOPEA_ANDROID_ARCH:-aarch64}
case "$arch" in
  aarch64) abi=arm64-v8a; label=arm64; default_stage=/tmp/hopea-android-src ;;
  armv7) abi=armeabi-v7a; label=armv7; default_stage=/tmp/hopea-android-armv7-src ;;
  x86_64) abi=x86_64; label=x86_64; default_stage=/tmp/hopea-android-x86_64-src ;;
  *) echo 'HOPEA_ANDROID_ARCH must be aarch64, armv7 or x86_64.' >&2; exit 1 ;;
esac
stage=${1:-$default_stage}
mkdir -p "$stage"
stage=$(cd -- "$stage" && pwd)
if [[ "$stage" == "$repo" || "$stage" == "$repo/"* ]]; then
  echo 'Use a separate Android staging directory outside the source tree.' >&2
  exit 1
fi
if [[ ! -f "$repo/_builddir/src/gtk/window.ui" ]]; then
  echo 'Build the desktop resources first: nix-shell nix/shell.nix --run "meson compile -C _builddir"' >&2
  exit 1
fi
# Copy only project sources. Browser profiles, local music and cookies are excluded.
tar -C "$repo" --exclude=target --exclude=__pycache__ -cf - \
  src common node spotify vendor data po tests build-aux Cargo.toml Cargo.lock meson.build meson_options.txt \
  | tar -C "$stage" -xf -
patch -d "$stage" -p1 --batch < "$repo/build-aux/android/port.patch"
cp "$repo"/_builddir/src/gtk/*.ui "$stage/src/gtk/"
cp "$repo/build-aux/android/cargo.sh" "$stage/android-cargo.sh"
cp "$repo/build-aux/android/main.c" "$stage/src/android-main.c"
cp "$repo/build-aux/android/android.cross" "$stage/hopea-rust.cross"
cp "$repo/build-aux/android/hopea.xml" "$stage/hopea-android.xml"
sed -i "s/aarch64/$arch/g" "$stage/hopea-android.xml"
chmod +x "$stage/android-cargo.sh"
if [[ "${HOPEA_ANDROID_REBUILD_TOOLCHAIN:-0}" == 1 ]] || ! docker image inspect hopea-android-toolchain >/dev/null 2>&1; then
  docker build -t hopea-android-toolchain "$repo/build-aux/android"
fi
docker run --init --rm --env "HOPEA_ANDROID_ARCH=$arch" --mount "type=bind,src=$stage,dst=/work" --workdir /work \
  hopea-android-toolchain bash /work/build-aux/android/container-build.sh
mkdir -p "$repo/dist"
cp "$stage/.pixiewood/android/app/build/outputs/apk/debug/app-$abi-debug.apk" \
  "$repo/dist/Hopea-android-$label-preview.apk"
sha256sum "$repo/dist/Hopea-android-$label-preview.apk"
