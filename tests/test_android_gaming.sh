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
# Compile every Android bridge class against the real SDK, not mock APIs.
javac -Xlint:deprecation -cp "$sdk/platforms/android-36/android.jar" -d "$work/android" \
  "$root"/build-aux/android/java/io/hopea/hopea/*.java
python3 "$root/tests/test_android_manifest.py"
