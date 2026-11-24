#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd /work
arch=${HOPEA_ANDROID_ARCH:-aarch64}
case "$arch" in aarch64|armv7|x86_64) ;; *) echo 'Unsupported Android architecture' >&2; exit 1 ;; esac
build_dir=.pixiewood/bin-$arch
export CARGO_BUILD_JOBS="${HOPEA_BUILD_JOBS:-4}"
bash tests/test_android_gaming.sh
bash tests/test_android_build.sh --staged
python3 build-aux/android/prepare-compat.py
# A different API changes libc symbol availability. Never mix API 31 objects
# from a previous preview with the new API 28 libraries.
compiler=android28-clang
if [[ $arch == armv7 ]]; then
  compiler=androideabi28-clang
  # Existing cached toolchain images predate ARMv7 support.
  rustup target add armv7-linux-androideabi
fi
if [[ -f "$build_dir/build.ninja" ]] && ! grep -q "$compiler" "$build_dir/build.ninja"; then
  meson setup --wipe "$build_dir"
fi
/opt/pixiewood/pixiewood prepare -s /opt/android -a /opt/mini-studio hopea-android.xml
cp src/gtk/*.ui "$build_dir/src/gtk/"
if ! grep -q 'Hopea: official client' subprojects/gtk/gdk/android/glue/java/org/gtk/android/ToplevelActivity.java; then
  if ! grep -q 'Hopea: prefer' subprojects/gtk/gdk/android/glue/java/org/gtk/android/ToplevelActivity.java; then
    patch --batch --fuzz=0 -d subprojects/gtk -p1 < build-aux/android/app-links.patch
  fi
  if ! grep -q 'Hopea: Yandex Music' subprojects/gtk/gdk/android/glue/java/org/gtk/android/ToplevelActivity.java; then
    patch --batch --fuzz=0 -d subprojects/gtk -p1 < build-aux/android/yandex-app-links.patch
  fi
  patch --batch --fuzz=0 -d subprojects/gtk -p1 < build-aux/android/service-links.patch
fi
if ! grep -q 'Hopea uses private' subprojects/gtk/gdk/android/glue/java/org/gtk/android/RuntimeApplication.java; then
  patch -d subprojects/gtk -p1 < build-aux/android/runtime-paths.patch
fi
if ! grep -q 'hopeaPreserveUserFiles' subprojects/gtk/gdk/android/glue/java/org/gtk/android/RuntimeApplication.java; then
  patch --batch --fuzz=0 -d subprojects/gtk -p1 < build-aux/android/storage-preservation.patch
fi
if ! grep -q 'Hopea: the disposable importer' subprojects/gtk/gdk/android/glue/java/org/gtk/android/RuntimeApplication.java; then
  patch --batch --fuzz=0 -d subprojects/gtk -p1 < build-aux/android/import-runtime.patch
fi
if ! grep -q 'Hopea: dynamic system colors' subprojects/gtk/gdk/android/gdkandroidinit.c; then
  patch --batch --fuzz=0 -d subprojects/gtk -p1 < build-aux/android/gtk-compat.patch
fi
if [[ ! -f subprojects/gtk/gdk/android/gdkandroidinput-compat-private.h ]]; then
  patch --batch --fuzz=0 -d subprojects/gtk -p1 < build-aux/android/gtk-input-compat.patch
fi
if ! grep -qE 'Hopea: the legacy callback|Hopea: API 28 provides' subprojects/gtk/gdk/android/gdkandroidchoreographersource.c; then
  patch --batch --fuzz=0 -d subprojects/gtk -p1 < build-aux/android/choreographer-compat.patch
fi
if ! grep -q 'atomic_uint_least64_t presentation_time' subprojects/gtk/gdk/android/gdkandroidchoreographersource.c; then
  patch --batch --fuzz=0 -d subprojects/gtk -p1 < build-aux/android/choreographer-armv7.patch
fi
if ! grep -q 'Hopea: GTK document-picker bridge' subprojects/gtk/gdk/android/glue/java/org/gtk/android/ToplevelActivity.java; then
  patch --batch --fuzz=0 -d subprojects/gtk -p1 < build-aux/android/gtk-runtime-compat.patch
fi
runtime_target=$arch-linux-android
if ! grep -q 'Hopea: let Android adjust media volume' subprojects/gtk/gdk/android/glue/java/org/gtk/android/ToplevelActivity.java; then
  patch --batch --fuzz=0 -d subprojects/gtk -p1 < build-aux/android/gtk-media-keys.patch
fi
if ! grep -q 'Hopea: event object identity changes within a drag' subprojects/gtk/gdk/android/glue/java/org/gtk/android/ToplevelActivity.java; then
  patch --batch --fuzz=0 -d subprojects/gtk -p1 < build-aux/android/gtk-touch-sequence.patch
fi
if [[ $arch == armv7 ]]; then runtime_target=armv7a-linux-androideabi; fi
ANDROID_RUNTIME_CC="/opt/android/ndk/28.2.13676358/toolchains/llvm/prebuilt/linux-x86_64/bin/${runtime_target}28-clang" \
  python3 tests/test_android_runtime.py subprojects/gtk
python3 tests/test_android_frame_clock.py subprojects/gtk/gdk/android/gdkandroidchoreographersource.c
if ! grep -q hopea_fdk_security_log subprojects/fdk-aac-2.0.3/libSBRdec/src/lpp_tran.cpp; then
  patch -d subprojects/fdk-aac-2.0.3 -p1 < build-aux/android/fdk-ndk.patch
fi
if ! grep -q 'Android packages unversioned' subprojects/gstreamer/subprojects/gst-plugins-good/ext/soup/gstsouploader.c; then
  patch --batch --fuzz=0 -d subprojects/gstreamer -p1 < build-aux/android/gstreamer-soup.patch
fi
if ! grep -q 'g_libintl_use_backend' subprojects/proxy-libintl-0.5/libintl.c; then
  patch --batch --fuzz=0 -d subprojects/proxy-libintl-0.5 -p1 < build-aux/android/intl-backend.patch
fi
meson setup --reconfigure "$build_dir" \
  -Dgdk-pixbuf:android=disabled -Dgdk-pixbuf:png=enabled -Dgdk-pixbuf:jpeg=enabled \
  -Dfontconfig:tests=disabled -Dfontconfig:cache-build=disabled \
  -Dgstreamer:tests=disabled -Dgstreamer:examples=disabled \
  -Dgst-plugins-base:tests=disabled -Dgst-plugins-base:examples=disabled \
  -Dgst-plugins-good:tests=disabled -Dgst-plugins-good:examples=disabled \
  -Dgst-plugins-good:flac=enabled \
  -Dgst-plugins-bad:tests=disabled -Dgst-plugins-bad:examples=disabled \
  -Dgstreamer-full:libav=enabled \
  -Dgst-plugins-ugly:tests=disabled
meson configure "$build_dir" -Dgst-libav:tests=disabled
ninja -C "$build_dir" -j "${HOPEA_BUILD_JOBS:-4}"
/opt/pixiewood/pixiewood generate
/opt/pixiewood/pixiewood build --skip-gradle
bash build-aux/android/package.sh
