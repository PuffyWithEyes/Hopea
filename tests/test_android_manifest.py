# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Build configuration check; never reads a device or user profile."""
import subprocess
import importlib.util
import os
import re
import sys
import tempfile
from pathlib import Path
from xml.etree import ElementTree as ET

source = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("privacy", source / "build-aux/android/check-privacy.py")
privacy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(privacy)
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
    assert xml.find("application").get(name) == "io.hopea.hopea.HopeaApplication"
    permissions = [node.get(name) for node in xml.findall("uses-permission")]
    assert permissions.count("android.permission.PACKAGE_USAGE_STATS") == 1
    assert "android.permission.QUERY_ALL_PACKAGES" not in permissions
    categories = [node.get(name) for node in xml.findall("queries/intent/category")]
    assert categories == ["android.intent.category.LAUNCHER", "android.intent.category.LEANBACK_LAUNCHER",
                          "android.intent.category.BROWSABLE"]
    assert [node.get("{http://schemas.android.com/apk/res/android}scheme")
            for node in xml.findall("queries/intent/data")] == ["https"]
    assert len(xml.findall("application/service")) == 2
    importer = next(s for s in xml.findall("application/service") if s.get(name).endswith('.ImportService'))
    assert importer.get('{http://schemas.android.com/apk/res/android}exported') == 'false'
    assert importer.get('{http://schemas.android.com/apk/res/android}process') == ':import'
    login = next(s for s in xml.findall("application/activity") if s.get(name).endswith('.VkSignInActivity'))
    assert login.get('{http://schemas.android.com/apk/res/android}exported') == 'false'
    privacy.check(xml)
    # A dependency may add these during manifest merging. Never silently ship it.
    for tag, attributes in [
        ("uses-permission", {name: "android.permission.READ_MEDIA_IMAGES"}),
        ("uses-permission-sdk-23", {name: "android.permission.READ_EXTERNAL_STORAGE"}),
    ]:
        added = ET.SubElement(xml, tag, attributes)
        try:
            privacy.check(xml)
        except ValueError:
            pass
        else:
            raise AssertionError("Unreviewed media permission accepted")
        xml.remove(added)
    app_xml = xml.find("application")
    added = ET.SubElement(app_xml, "provider", {name: "untrusted.ExportedProvider",
        '{http://schemas.android.com/apk/res/android}exported': 'true'})
    try:
        privacy.check(xml)
    except ValueError:
        pass
    else:
        raise AssertionError("Exported provider accepted")
    app_xml.remove(added)
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
        gradle = (app / 'build.gradle').read_text()
        assert gradle.count('details.useVersion("3.11.18-hopea1")') == 1
        assert gradle.count('includeModule("com.chaquo.python", "target")') == 1
        assert '../../.android-python-runtime/maven' in gradle
        assert int(re.search(r'versionCode (\d+)', gradle).group(1)) > 20261117
print("Android usage-access manifest: passed")
