#!/usr/bin/env bash
set -euo pipefail
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
cd /work
arch=${HOPEA_ANDROID_ARCH:-aarch64}
case "$arch" in
  aarch64) abi=arm64-v8a ;;
  x86_64) abi=x86_64 ;;
  armv7) abi=armeabi-v7a ;;
  *) echo 'Unsupported Android architecture' >&2; exit 1 ;;
esac
app=.pixiewood/android/app
bash build-aux/android/build-import-runtime.sh /work/.android-python-runtime "$abi"
cp data/icons/cat-kz/512.png "$app/src/main/res/drawable/hopea_cat.png"
cp build-aux/android/launcher-foreground.xml "$app/src/main/res/drawable/ic_launcher_foreground.xml"
python3 build-aux/android/configure-package.py
cp -r build-aux/android/java/. "$app/src/main/java/"
mkdir -p "$app/src/main/python"
cp src/library/service_auth.py src/library/service_import.py build-aux/android/android_import.py "$app/src/main/python/"
cp build-aux/android/import-requirements.txt "$app/import-requirements.txt"
cp build-aux/import-constraints.txt "$app/import-constraints.txt"
sed -i 's@<string name="app_name">.*</string>@<string name="app_name">Hopea</string>@' "$app"/src/main/res/values*/strings.xml
cp data/io.hopea.Hopea.gschema.xml .pixiewood/root/share/glib-2.0/schemas/

sdk_tools=/opt/android/ndk/28.2.13676358/toolchains/llvm/prebuilt/linux-x86_64
native_libs=/work/.pixiewood/root/lib/$abi
assets=/work/.pixiewood/android/app/src/main/assets
mkdir -p "$assets/share/licenses/android-python"
cp /work/.android-python-runtime/licenses/*.txt "$assets/share/licenses/android-python/"
# Pixiewood collects the application's dependency closure, which does not
# include Meson's independent gettext targets. Ship every discovered catalog.
python3 build-aux/android/package-translations.py ".pixiewood/bin-$arch/po" "$assets/share/locale"
# Do not depend on OEM font substitutions/metrics for spaces in UI labels.
mkdir -p "$assets/share/fonts" "$assets/etc/fonts" "$assets/share/licenses/fonts-dejavu"
cp /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf /usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf \
  /usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf "$assets/share/fonts/"
cp /usr/share/doc/fonts-dejavu-core/copyright "$assets/share/licenses/fonts-dejavu/"
mkdir -p "$assets/share/licenses/android-cpufeatures"
cp /opt/android/ndk/28.2.13676358/sources/android/cpufeatures/NOTICE \
  "$assets/share/licenses/android-cpufeatures/"
cp build-aux/android/fonts.conf "$assets/etc/fonts/fonts.conf"
glib-compile-schemas --targetdir "$assets/share/glib-2.0/schemas" /work/.pixiewood/root/share/glib-2.0/schemas
python3 build-aux/android/fingerprint-assets.py "$assets"
# Android only loads native libraries at the top of its ABI directory.
find "$native_libs" -mindepth 2 -type f -name '*.so' -exec cp -t "$native_libs" {} +
sysroot_target=$arch-linux-android
if [[ $arch == armv7 ]]; then sysroot_target=arm-linux-androideabi; fi
cp "$sdk_tools/sysroot/usr/lib/$sysroot_target/libc++_shared.so" "$native_libs/"
python3 build-aux/android/check-audio.py "$native_libs" "$sdk_tools/bin/llvm-readelf"
find "$native_libs" -maxdepth 1 -type f -name '*.so' -exec "$sdk_tools/bin/llvm-strip" --strip-unneeded {} +
export ANDROID_HOME=/opt/android
export ANDROID_USER_HOME=/work/.android-user
export GRADLE_USER_HOME=/work/.gradle-user
mkdir -p "$ANDROID_USER_HOME"
chmod 700 "$ANDROID_USER_HOME"
cd /work/.pixiewood/android
./gradlew --no-daemon --max-workers=4 assembleDebug
python3 /work/build-aux/android/check-privacy.py \
  app/build/intermediates/merged_manifest/debug/processDebugMainManifest/AndroidManifest.xml
python3 /work/build-aux/android/check-import-runtime.py /work/.android-python-runtime \
  app/build/intermediates/merged_native_libs/debug/mergeDebugNativeLibs/out/lib \
  app/build/intermediates/assets/debug/mergeDebugAssets/chaquopy "$abi"
apk=app/build/outputs/apk/debug/app-$abi-debug.apk
/opt/android/build-tools/36.0.0/apksigner verify "$apk"
/opt/android/build-tools/36.0.0/zipalign -c -P 16 4 "$apk"
