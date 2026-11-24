#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
sdk=${ANDROID_HOME:-/opt/android}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
javac -d "$work/classes" \
  "$root/build-aux/android/java/io/hopea/hopea/GameActivityState.java" \
  "$root/tests/GameActivityStateTest.java"
java -ea -cp "$work/classes" io.hopea.hopea.GameActivityStateTest
python3 "$root/tests/test_android_app_links.py"
python3 "$root/tests/test_android_storage.py"
python3 "$root/tests/test_android_crash_report.py"
python3 "$root/tests/test_android_language.py"
python3 "$root/tests/test_android_keyboard.py"
# These checks run before Pixiewood downloads GTK. Supply only its Application
# superclass contract; Android APIs come from the real SDK. Gradle subsequently
# compiles HopeaApplication together with the actual GTK Java sources.
cat > "$work/RuntimeApplication.java" <<'JAVA'
package org.gtk.android;
public class RuntimeApplication extends android.app.Application {}
JAVA
# Compile the import worker against the same real Java API as the APK. It is
# not part of android.jar; the final Gradle build resolves it through Chaquopy.
curl --fail --location --proto '=https' --tlsv1.2 --retry 3 \
  https://repo.maven.apache.org/maven2/com/chaquo/python/runtime/chaquopy_java/17.2.0/chaquopy_java-17.2.0.jar \
  -o "$work/chaquopy.jar"
printf '%s  %s\n' 9cf6f66ba74f89e334f9da3ffade49cfd3f48ed06030557299087216d7054c35 \
  "$work/chaquopy.jar" | sha256sum -c -
javac -Xlint:deprecation -cp "$sdk/platforms/android-36/android.jar:$work/chaquopy.jar" -d "$work/android" \
  "$work/RuntimeApplication.java" \
  "$root"/build-aux/android/java/io/hopea/hopea/*.java
python3 "$root/tests/test_android_manifest.py"
