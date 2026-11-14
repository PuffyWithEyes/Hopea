#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd /work
bash tests/test_android_gaming.sh
/opt/pixiewood/pixiewood prepare -s /opt/android -a /opt/mini-studio hopea-android.xml
cp src/gtk/*.ui .pixiewood/bin-aarch64/src/gtk/
if ! grep -q 'Hopea: prefer' subprojects/gtk/gdk/android/glue/java/org/gtk/android/ToplevelActivity.java; then
  patch -d subprojects/gtk -p1 < build-aux/android/app-links.patch
fi
if ! grep -q 'Hopea uses private' subprojects/gtk/gdk/android/glue/java/org/gtk/android/RuntimeApplication.java; then
  patch -d subprojects/gtk -p1 < build-aux/android/runtime-paths.patch
fi
if ! grep -q hopea_fdk_security_log subprojects/fdk-aac-2.0.3/libSBRdec/src/lpp_tran.cpp; then
  patch -d subprojects/fdk-aac-2.0.3 -p1 < build-aux/android/fdk-ndk.patch
fi
meson configure .pixiewood/bin-aarch64 \
  -Dfontconfig:tests=disabled -Dfontconfig:cache-build=disabled \
  -Dgstreamer:tests=disabled -Dgstreamer:examples=disabled \
  -Dgst-plugins-base:tests=disabled -Dgst-plugins-base:examples=disabled \
  -Dgst-plugins-good:tests=disabled -Dgst-plugins-good:examples=disabled \
  -Dgst-plugins-bad:tests=disabled -Dgst-plugins-bad:examples=disabled \
  -Dgst-plugins-ugly:tests=disabled
ninja -C .pixiewood/bin-aarch64 -j 8
/opt/pixiewood/pixiewood generate
/opt/pixiewood/pixiewood build --skip-gradle
bash build-aux/android/package.sh
