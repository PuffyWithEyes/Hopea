#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise the patched GTK activity's dispatch logic with recording Android stubs.

This tests intent selection and fallback, not installed-app URL support on a device.
"""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

STUBS = r'''
import java.net.URI;
import java.util.ArrayList;
class Bundle {}
class Uri {
    URI value;
    Uri(String value) { this.value = URI.create(value); }
    String getHost() { return value.getHost(); }
    String getScheme() { return value.getScheme(); }
    String getPath() { return value.getPath(); }
}
class Intent {
    static final String ACTION_VIEW = "view", CATEGORY_BROWSABLE = "browsable";
    static final int FLAG_ACTIVITY_REQUIRE_NON_BROWSER = 1;
    Uri uri;
    String pkg, component, action = ACTION_VIEW;
    int flags;
    Intent(String url) { uri = new Uri(url); }
    Intent(Intent other) {
        uri = other.uri; pkg = other.pkg; component = other.component;
        action = other.action; flags = other.flags;
    }
    Uri getData() { return uri; }
    String getAction() { return action; }
    String getComponent() { return component; }
    String getPackage() { return pkg; }
    void setPackage(String value) { pkg = value; }
    void addCategory(String value) {}
    void addFlags(int value) { flags |= value; }
}
class RecordingActivity {
    ArrayList<Intent> calls = new ArrayList<>();
    int failure;
    public void startActivity(Intent intent, Bundle options) {
        calls.add(intent);
        if (intent.pkg != null || intent.flags != 0) {
            if (failure == 1) throw new android.content.ActivityNotFoundException();
            if (failure == 2) throw new SecurityException();
        }
    }
}
'''

CHECKS = r'''
class DispatchTest {
    public static void main(String[] args) {
        for (String url : new String[] {"https://ya.ru/device",
                "https://oauth.yandex.ru/device", "https://oauth.yandex.com/device/",
                "https://music.yandex.ru/", "https://music.yandex.com/"}) {
            for (int failure = 0; failure <= 2; failure++) {
                ToplevelActivity activity = new ToplevelActivity();
                activity.failure = failure;
                Intent original = new Intent(url);
                activity.startActivity(original, null);
                assert activity.calls.size() == (failure == 0 ? 1 : 2);
                Intent first = activity.calls.get(0);
                assert "ru.yandex.music".equals(first.pkg);
                assert first.uri == original.uri : "Confirmation URL was replaced";
                assert first.flags == 0 : "Explicit app dispatch must work before API 30";
                assert original.pkg == null && original.flags == 0;
                if (failure != 0) assert activity.calls.get(1) == original;
            }
        }
        for (String url : new String[] {"https://vk.ru/audio", "https://ya.ru.evil.test/device",
                "https://ya.ru/other", "https://music.yandex.ru.evil.test/"}) {
            ToplevelActivity activity = new ToplevelActivity();
            activity.failure = 1;
            Intent original = new Intent(url);
            activity.startActivity(original, null);
            assert activity.calls.size() == 2;
            assert activity.calls.get(0).pkg == null;
            assert activity.calls.get(0).flags == Intent.FLAG_ACTIVITY_REQUIRE_NON_BROWSER;
            assert activity.calls.get(1) == original;
        }
        ToplevelActivity activity = new ToplevelActivity();
        Intent explicit = new Intent("https://ya.ru/device");
        explicit.setPackage("chosen.app");
        activity.startActivity(explicit, null);
        assert activity.calls.size() == 1 && activity.calls.get(0) == explicit;
    }
}
'''


class AppLinksTest(unittest.TestCase):
    def test_dispatch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / 'gdk/android/glue/java/org/gtk/android/ToplevelActivity.java'
            target.parent.mkdir(parents=True)
            patch = (ROOT / 'build-aux/android/app-links.patch').read_text()
            method = '\n'.join(line[1:] for line in patch.splitlines()
                               if line.startswith('+') and not line.startswith('+++'))
            target.write_text(STUBS + '\nclass ToplevelActivity extends RecordingActivity {\n'
                              + method + '\n}\n' + CHECKS)
            subprocess.run(['patch', '--batch', '--fuzz=0', '-p1', '-i',
                            str(ROOT / 'build-aux/android/yandex-app-links.patch')],
                           cwd=root, check=True)
            exception = root / 'android/content/ActivityNotFoundException.java'
            exception.parent.mkdir(parents=True)
            exception.write_text('package android.content;\n'
                                 'public class ActivityNotFoundException extends RuntimeException {}\n')
            subprocess.run(['javac', '-d', str(root / 'classes'), str(exception), str(target)],
                           check=True)
            subprocess.run(['java', '-ea', '-cp', str(root / 'classes'), 'DispatchTest'], check=True)


if __name__ == '__main__':
    unittest.main()
