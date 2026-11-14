#!/bin/bash
set -euo pipefail
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
cd /work
app=.pixiewood/android/app
cp data/icons/cat-kz/512.png "$app/src/main/res/drawable/hopea_cat.png"
cp build-aux/android/launcher-foreground.xml "$app/src/main/res/drawable/ic_launcher_foreground.xml"
python3 build-aux/android/configure-package.py
cp -r build-aux/android/java/. "$app/src/main/java/"
sed -i 's@<string name="app_name">.*</string>@<string name="app_name">Hopea</string>@' "$app"/src/main/res/values*/strings.xml
cp data/io.hopea.Hopea.gschema.xml .pixiewood/root/share/glib-2.0/schemas/

sdk_tools=/opt/android/ndk/28.2.13676358/toolchains/llvm/prebuilt/linux-x86_64
native_libs=/work/.pixiewood/root/lib/arm64-v8a
assets=/work/.pixiewood/android/app/src/main/assets
glib-compile-schemas --targetdir "$assets/share/glib-2.0/schemas" /work/.pixiewood/root/share/glib-2.0/schemas
# Android only loads native libraries at the top of its ABI directory.
find "$native_libs" -mindepth 2 -type f -name '*.so' -exec cp -t "$native_libs" {} +
cp "$sdk_tools/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so" "$native_libs/"
find "$native_libs" -maxdepth 1 -type f -name '*.so' -exec "$sdk_tools/bin/llvm-strip" --strip-unneeded {} +
export ANDROID_HOME=/opt/android
export ANDROID_USER_HOME=/work/.android-user
export GRADLE_USER_HOME=/work/.gradle-user
mkdir -p "$ANDROID_USER_HOME"
chmod 700 "$ANDROID_USER_HOME"
cd /work/.pixiewood/android
./gradlew --no-daemon --max-workers=4 assembleDebug
apk=app/build/outputs/apk/debug/app-arm64-v8a-debug.apk
/opt/android/build-tools/36.0.0/apksigner verify "$apk"
/opt/android/build-tools/36.0.0/zipalign -c -P 16 4 "$apk"
