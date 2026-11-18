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
# These checks run before Pixiewood downloads GTK. Supply only its Application
# superclass contract; Android APIs come from the real SDK. Gradle subsequently
# compiles HopeaApplication together with the actual GTK Java sources.
cat > "$work/RuntimeApplication.java" <<'JAVA'
package org.gtk.android;
public class RuntimeApplication extends android.app.Application {}
JAVA
javac -Xlint:deprecation -cp "$sdk/platforms/android-36/android.jar" -d "$work/android" \
  "$work/RuntimeApplication.java" \
  "$root"/build-aux/android/java/io/hopea/hopea/*.java
python3 "$root/tests/test_android_manifest.py"
