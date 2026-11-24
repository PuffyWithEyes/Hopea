# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Reject unexpected permissions/components in the *merged* APK manifest."""
import sys
from xml.etree import ElementTree as ET

ANDROID = "{http://schemas.android.com/apk/res/android}"
PERMISSIONS = {"android.permission." + name for name in (
    "REORDER_TASKS", "INTERNET", "ACCESS_NETWORK_STATE", "FOREGROUND_SERVICE",
    "FOREGROUND_SERVICE_MEDIA_PLAYBACK", "WAKE_LOCK", "PACKAGE_USAGE_STATS",
)}


def check(root):
    for node in root:
        if node.tag.startswith("uses-permission"):
            permission = node.get(ANDROID + "name")
            if permission not in PERMISSIONS:
                raise ValueError(f"Unreviewed Android permission: {permission}")
    application = root.find("application")
    if application is None or application.get(ANDROID + "allowBackup") != "false":
        raise ValueError("App data must be excluded from Android backup")
    for node in application:
        if node.tag not in ("activity", "activity-alias", "service", "receiver", "provider"):
            continue
        name = node.get(ANDROID + "name")
        if name == "org.gtk.android.ToplevelActivity" and node.tag == "activity":
            continue  # Public launcher; import components must remain private.
        if node.get(ANDROID + "exported") != "false":
            raise ValueError(f"Unreviewed exported Android component: {name}")


if __name__ == "__main__":
    check(ET.parse(sys.argv[1]).getroot())
    print("Merged Android permissions and private components: passed")
