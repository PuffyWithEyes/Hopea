# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Idempotent build-time manifest configuration; never runs on the phone."""
from pathlib import Path
import re
import xml.etree.ElementTree as ET

app = Path('.pixiewood/android/app')
android = 'http://schemas.android.com/apk/res/android'
ET.register_namespace('android', android)
key = lambda name: f'{{{android}}}{name}'
path = app / 'src/main/AndroidManifest.xml'
tree = ET.parse(path)
root = tree.getroot()
application = root.find('application')
application.set(key('name'), 'io.hopea.hopea.HopeaApplication')
application.set(key('allowBackup'), 'false')
application.set(key('extractNativeLibs'), 'true')
# Do not exclude tablets, foldables or devices without touch input. Playback
# requires neither a camera, telephony, GPS, nor Google Play Services.
screens = root.find('supports-screens')
if screens is None:
    screens = ET.SubElement(root, 'supports-screens')
for name in ['smallScreens', 'normalScreens', 'largeScreens', 'xlargeScreens', 'anyDensity']:
    screens.set(key(name), 'true')
for name in ['android.hardware.touchscreen', 'android.hardware.faketouch']:
    feature = next((f for f in root.findall('uses-feature') if f.get(key('name')) == name), None)
    if feature is None:
        feature = ET.SubElement(root, 'uses-feature', {key('name'): name})
    feature.set(key('required'), 'false')
permissions = ['INTERNET', 'ACCESS_NETWORK_STATE', 'FOREGROUND_SERVICE',
               'FOREGROUND_SERVICE_MEDIA_PLAYBACK', 'WAKE_LOCK', 'PACKAGE_USAGE_STATS']
seen = set()
for permission in list(root.findall('uses-permission')):
    name = permission.get(key('name'))
    if name in seen:
        root.remove(permission)
    seen.add(name)
for name in permissions:
    name = 'android.permission.' + name
    if name not in seen:
        ET.SubElement(root, 'uses-permission', {key('name'): name})
# Query launchable apps and URL handlers; do not request QUERY_ALL_PACKAGES.
queries = root.find('queries')
if queries is None:
    queries = ET.SubElement(root, 'queries')
for category in ['LAUNCHER', 'LEANBACK_LAUNCHER']:
    category = 'android.intent.category.' + category
    if not any(i.find('category') is not None and i.find('category').get(key('name')) == category
               and i.find('action') is not None and i.find('action').get(key('name')) == 'android.intent.action.MAIN'
               for i in queries.findall('intent')):
        intent = ET.SubElement(queries, 'intent')
        ET.SubElement(intent, 'action', {key('name'): 'android.intent.action.MAIN'})
        ET.SubElement(intent, 'category', {key('name'): category})
if not any(i.find('action') is not None
           and i.find('action').get(key('name')) == 'android.intent.action.VIEW'
           and i.find('data') is not None
           and i.find('data').get(key('scheme')) == 'https'
           for i in queries.findall('intent')):
    intent = ET.SubElement(queries, 'intent')
    ET.SubElement(intent, 'action', {key('name'): 'android.intent.action.VIEW'})
    ET.SubElement(intent, 'category', {key('name'): 'android.intent.category.BROWSABLE'})
    ET.SubElement(intent, 'data', {key('scheme'): 'https'})
name = 'io.hopea.hopea.PlaybackService'
if not any(s.get(key('name')) == name for s in application.findall('service')):
    ET.SubElement(application, 'service', {key('name'): name,
        key('exported'): 'false', key('foregroundServiceType'): 'mediaPlayback'})
name = 'io.hopea.hopea.ImportService'
if not any(s.get(key('name')) == name for s in application.findall('service')):
    ET.SubElement(application, 'service', {key('name'): name,
        key('exported'): 'false', key('process'): ':import', key('stopWithTask'): 'true'})
name = 'io.hopea.hopea.VkSignInActivity'
if not any(s.get(key('name')) == name for s in application.findall('activity')):
    ET.SubElement(application, 'activity', {key('name'): name,
        key('exported'): 'false', key('excludeFromRecents'): 'true',
        key('theme'): '@android:style/Theme.Material.NoActionBar',
        key('configChanges'): 'orientation|screenSize|keyboardHidden'})
tree.write(path, encoding='utf-8', xml_declaration=True)
path = app / 'build.gradle'
text = path.read_text()
if 'com.chaquo.python' not in text:
    text = text.replace('plugins {', 'plugins {\n    id "com.chaquo.python" version "17.2.0"', 1)
    text += '''
chaquopy {
    defaultConfig {
        // Python 3.11 retains ARMv7 support on Android 9.
        version = "3.11"
        pip { install("-r", "import-requirements.txt") }
    }
}
'''
if 'useLegacyPackaging' not in text:
    text = text.replace('android {', 'android {\n    packaging { jniLibs { useLegacyPackaging = true } }', 1)
text = re.sub(r'minSdk \d+', 'minSdk 28', text)
# Keep upgrades installable over the previous audio and diagnostic previews.
text = re.sub(r'versionCode \d+', 'versionCode 20261124', text)
text = re.sub(r'versionName "[^"]*"', 'versionName "2026.3.5-android-preview"', text)
path.write_text(text)

# The published Chaquopy target still contains CPython 3.11.17. Only resolve our
# source-built security update; never silently fall back to the older runtime.
text = text.split('\n// Hopea CPython security update\n', 1)[0]
text += '''
// Hopea CPython security update
buildscript.repositories {
    maven {
        url = rootProject.file("../../.android-python-runtime/maven").toURI()
        content { includeModule("com.chaquo.python", "target") }
    }
}
// Chaquopy 17.2 uses detached buildscript configurations: the project's
// configurations.configureEach does not reach them.
afterEvaluate {
    plugins.getPlugin("com.chaquo.python").configs.values().each { config ->
        config.resolutionStrategy.eachDependency { details ->
            if (details.requested.group == "com.chaquo.python" && details.requested.name == "target") {
                details.useVersion("3.11.18-hopea1")
            }
        }
    }
}
'''
path.write_text(text)
settings = app.parent / 'settings.gradle'
if settings.exists():
    text = settings.read_text().split('\n// Hopea CPython security update\n', 1)[0]
    settings.write_text(text)
