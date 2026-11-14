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
application.set(key('allowBackup'), 'false')
application.set(key('extractNativeLibs'), 'true')
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
# Classify launchable apps only; do not request QUERY_ALL_PACKAGES.
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
name = 'io.hopea.hopea.PlaybackService'
if not any(s.get(key('name')) == name for s in application.findall('service')):
    ET.SubElement(application, 'service', {key('name'): name,
        key('exported'): 'false', key('foregroundServiceType'): 'mediaPlayback'})
tree.write(path, encoding='utf-8', xml_declaration=True)
path = app / 'build.gradle'
text = path.read_text()
if 'useLegacyPackaging' not in text:
    text = text.replace('android {', 'android {\n    packaging { jniLibs { useLegacyPackaging = true } }', 1)
text = re.sub(r'versionCode \d+', 'versionCode 20261114', text)
text = re.sub(r'versionName "[^"]*"', 'versionName "2026.11-android-preview"', text)
path.write_text(text)
