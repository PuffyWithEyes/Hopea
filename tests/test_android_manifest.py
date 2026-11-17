# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Build configuration check; never reads a device or user profile."""
import subprocess
import os
import sys
import tempfile
from pathlib import Path
from xml.etree import ElementTree as ET

source = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    app = root / ".pixiewood/android/app"
    main = app / "src/main"
    main.mkdir(parents=True)
    manifest = main / "AndroidManifest.xml"
    manifest.write_text('<manifest xmlns:android="http://schemas.android.com/apk/res/android"><application/></manifest>')
    (app / "build.gradle").write_text('android {\n minSdk 31\n versionCode 1\n versionName "test"\n}\n')
    for _ in range(2):
        subprocess.run([sys.executable, str(source / "build-aux/android/configure-package.py")], cwd=root, check=True)
    xml = ET.parse(manifest).getroot()
    name = "{http://schemas.android.com/apk/res/android}name"
    permissions = [node.get(name) for node in xml.findall("uses-permission")]
    assert permissions.count("android.permission.PACKAGE_USAGE_STATS") == 1
    assert "android.permission.QUERY_ALL_PACKAGES" not in permissions
    categories = [node.get(name) for node in xml.findall("queries/intent/category")]
    assert categories == ["android.intent.category.LAUNCHER", "android.intent.category.LEANBACK_LAUNCHER",
                          "android.intent.category.BROWSABLE"]
    assert [node.get("{http://schemas.android.com/apk/res/android}scheme")
            for node in xml.findall("queries/intent/data")] == ["https"]
    assert len(xml.findall("application/service")) == 1
    required = "{http://schemas.android.com/apk/res/android}required"
    features = {node.get(name): node.get(required) for node in xml.findall("uses-feature")}
    assert features == {"android.hardware.touchscreen": "false", "android.hardware.faketouch": "false"}
    assert len(xml.findall("uses-feature")) == 2
    assert len(xml.findall("supports-screens")) == 1
    assert all(value == "true" for value in xml.find("supports-screens").attrib.values())
    for architecture, minimum in [('armv7', 28), ('aarch64', 28), ('x86_64', 28)]:
        subprocess.run([sys.executable, str(source / "build-aux/android/configure-package.py")],
                       cwd=root, check=True, env={**os.environ, 'HOPEA_ANDROID_ARCH': architecture})
        assert f'minSdk {minimum}\n' in (app / 'build.gradle').read_text()
print("Android usage-access manifest: passed")
