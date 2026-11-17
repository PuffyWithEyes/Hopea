#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Test the actual patched dispatch method with recording Android API doubles.

This covers fallback decisions, not installed clients' behavior on a device.
The bridge must also be compiled against the real Android SDK.
"""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

STUBS = r'''
import java.net.URI;
import java.util.*;
class Bundle {}
class Uri {
    String text;
    URI value;
    Uri(String text) { this.text = text; value = URI.create(text.equals("https://") ? "https://example.test" : text); }
    static Uri parse(String value) { return new Uri(value); }
    String getHost() { return value.getHost(); }
    String getScheme() { return value.getScheme(); }
    String getPath() { return value.getPath(); }
    String getUserInfo() { return value.getUserInfo(); }
    int getPort() { return value.getPort(); }
    String getQuery() { return value.getQuery(); }
    String getFragment() { return value.getFragment(); }
}
class ComponentName {
    String pkg, name;
    ComponentName(String pkg, String name) { this.pkg = pkg; this.name = name; }
}
class Intent {
    static final String ACTION_VIEW = "view", CATEGORY_BROWSABLE = "browsable", EXTRA_INITIAL_INTENTS = "initial";
    static final int FLAG_ACTIVITY_REQUIRE_NON_BROWSER = 1, FLAG_ACTIVITY_NEW_TASK = 2;
    Uri uri;
    String pkg, action = ACTION_VIEW;
    ComponentName component;
    int flags;
    Intent target;
    Intent[] initial;
    Intent(String url) { uri = new Uri(url); }
    Intent(String action, Uri uri) { this.action = action; this.uri = uri; }
    Intent(Intent other) {
        uri = other.uri; pkg = other.pkg; component = other.component;
        action = other.action; flags = other.flags;
    }
    Uri getData() { return uri; }
    String getAction() { return action; }
    ComponentName getComponent() { return component; }
    String getPackage() { return pkg; }
    Intent setPackage(String value) { pkg = value; return this; }
    Intent setComponent(ComponentName value) { component = value; return this; }
    void addCategory(String value) {}
    void addFlags(int value) { flags |= value; }
    static Intent createChooser(Intent target, String title) {
        Intent choice = new Intent("chooser", null); choice.target = target; return choice;
    }
    void putExtra(String key, Intent[] values) { initial = values; }
}
class ActivityInfo {
    String packageName, name = "Main";
    boolean exported = true, enabled = true;
    ActivityInfo(String pkg) { packageName = pkg; }
}
class ResolveInfo {
    ActivityInfo activityInfo;
    ResolveInfo(String pkg) { activityInfo = new ActivityInfo(pkg); }
}
class PackageManager {
    static final int MATCH_DEFAULT_ONLY = 1;
    ArrayList<ResolveInfo> browsers = new ArrayList<>(), clients = new ArrayList<>();
    ResolveInfo preferred;
    List<ResolveInfo> queryIntentActivities(Intent intent, int flags) {
        return intent.uri.text.equals("https://") ? browsers : clients;
    }
    ResolveInfo resolveActivity(Intent intent, int flags) { return preferred; }
}
class RecordingActivity {
    ArrayList<Intent> calls = new ArrayList<>();
    Set<String> missing = new HashSet<>(), forbidden = new HashSet<>();
    PackageManager pm = new PackageManager();
    PackageManager getPackageManager() { return pm; }
    String getPackageName() { return "hopea"; }
    public void startActivity(Intent intent, Bundle options) {
        calls.add(intent);
        String pkg = intent.component == null ? intent.pkg : intent.component.pkg;
        if (missing.contains(pkg)) throw new android.content.ActivityNotFoundException("missing");
        if (forbidden.contains(pkg)) throw new SecurityException();
    }
}
'''

CHECKS = r'''
class DispatchTest {
    static ToplevelActivity fixture(String official) {
        ToplevelActivity a = new ToplevelActivity();
        a.missing.add(official);
        a.pm.browsers.add(new ResolveInfo("browser"));
        a.pm.preferred = a.pm.browsers.get(0);
        return a;
    }
    static void plainExternal(Intent actual, Intent original) {
        assert actual.uri == original.uri : "The exact URL must be preserved";
        assert actual.flags == Intent.FLAG_ACTIVITY_NEW_TASK : "External task; no API-30-only flags";
        assert original.flags == 0 && original.pkg == null && original.component == null;
    }
    public static void main(String[] args) {
        String[][] services = {
            {"https://vk.ru/audio", "com.vkontakte.android"},
            {"https://vk.com/audio", "com.vkontakte.android"},
            {"https://m.vk.com/audio-123", "com.vkontakte.android"},
            {"https://music.yandex.ru/", "ru.yandex.music"},
            {"https://music.yandex.com/", "ru.yandex.music"},
            {"https://open.spotify.com/", "com.spotify.music"},
            {"https://music.youtube.com/library", "com.google.android.apps.youtube.music"}
        };
        for (String[] service : services) {
            Intent link = new Intent(service[0]);
            ToplevelActivity a = fixture(service[1]);
            a.missing.clear();
            a.pm.clients.add(new ResolveInfo("alternative"));
            a.startActivity(link, null);
            assert a.calls.size() == 1 && service[1].equals(a.calls.get(0).pkg);
            plainExternal(a.calls.get(0), link);

            // Missing or disabled official client, and one supported alternative.
            for (boolean denied : new boolean[] {false, true}) {
                a = fixture(service[1]);
                if (denied) { a.missing.clear(); a.forbidden.add(service[1]); }
                a.pm.clients.add(new ResolveInfo("alternative"));
                a.startActivity(link, null);
                assert a.calls.size() == 2;
                assert "alternative".equals(a.calls.get(1).component.pkg);
                plainExternal(a.calls.get(1), link);
            }
            a = fixture(service[1]);
            a.startActivity(link, null);
            assert a.calls.size() == 2 && "browser".equals(a.calls.get(1).pkg);
            plainExternal(a.calls.get(1), link);
        }
        Intent vk = new Intent("https://vk.ru/audio");
        ToplevelActivity a = fixture("com.vkontakte.android");
        a.pm.clients.add(new ResolveInfo("kate"));
        a.pm.clients.add(new ResolveInfo("sova"));
        a.pm.clients.add(new ResolveInfo("kate")); // Multiple activities of one client.
        a.pm.clients.add(new ResolveInfo("hopea"));
        a.pm.clients.add(new ResolveInfo("browser"));
        a.pm.clients.add(new ResolveInfo("com.vkontakte.android"));
        ResolveInfo privateHandler = new ResolveInfo("private");
        privateHandler.activityInfo.exported = false; a.pm.clients.add(privateHandler);
        ResolveInfo disabled = new ResolveInfo("disabled");
        disabled.activityInfo.enabled = false; a.pm.clients.add(disabled);
        ResolveInfo genericBrowser = new ResolveInfo("other-browser");
        a.pm.browsers.add(genericBrowser); a.pm.clients.add(genericBrowser);
        a.startActivity(vk, null);
        Intent chooser = a.calls.get(1);
        assert "chooser".equals(chooser.action);
        assert "kate".equals(chooser.target.component.pkg);
        assert chooser.initial.length == 1 && "sova".equals(chooser.initial[0].component.pkg);
        plainExternal(chooser.target, vk);
        plainExternal(chooser.initial[0], vk);

        a = fixture("com.vkontakte.android");
        a.pm.clients.add(new ResolveInfo("alternative"));
        a.forbidden.add("alternative");
        a.startActivity(vk, null);
        assert a.calls.size() == 3 && "browser".equals(a.calls.get(2).pkg);

        a = fixture("com.vkontakte.android");
        a.pm.preferred = null;
        a.pm.browsers.add(new ResolveInfo("browser-two"));
        a.startActivity(vk, null);
        assert "chooser".equals(a.calls.get(1).action);
        assert a.calls.get(1).initial.length == 1;

        // Confirmation pages must never be offered to arbitrary third-party handlers.
        for (String url : new String[] {"https://ya.ru/device", "https://oauth.yandex.ru/device",
                "https://oauth.yandex.com/device/"}) {
            a = fixture("ru.yandex.music");
            a.pm.clients.add(new ResolveInfo("untrusted-oauth-handler"));
            Intent link = new Intent(url);
            a.startActivity(link, null);
            assert a.calls.size() == 2 && "browser".equals(a.calls.get(1).pkg);
            plainExternal(a.calls.get(1), link);
        }
        for (String url : new String[] {"https://vk.ru.evil.test/audio", "https://vk.ru:8443/audio",
                "https://user@vk.ru/audio", "https://vk.ru/authorize", "file:///tmp/file",
                "https://ya.ru/device?code=private", "https://ya.ru/other", "http://vk.ru/audio"}) {
            a = new ToplevelActivity();
            Intent original = new Intent(url);
            a.startActivity(original, null);
            assert a.calls.size() == 1 && a.calls.get(0) == original : url;
        }
        a = new ToplevelActivity();
        Intent explicit = new Intent("https://vk.ru/audio").setPackage("chosen.app");
        a.startActivity(explicit, null);
        assert a.calls.size() == 1 && a.calls.get(0) == explicit;
        a = fixture("com.vkontakte.android");
        a.pm.browsers.clear(); a.pm.preferred = null;
        try { a.startActivity(vk, null); throw new AssertionError("Missing handlers must report failure"); }
        catch (android.content.ActivityNotFoundException expected) {}
        System.out.println("Service links: official, alternatives, browser, chooser and failure checks passed");
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
            target.write_text('class ToplevelActivity extends RecordingActivity {\n' + method + '\n}\n')
            for name in ('yandex-app-links.patch', 'service-links.patch'):
                subprocess.run(['patch', '--batch', '--fuzz=0', '-p1', '-i',
                                str(ROOT / 'build-aux/android' / name)], cwd=root, check=True)
            # Replace qualified API names with the recording doubles. Real SDK
            # compilation separately verifies platform API signatures.
            body = target.read_text().replace('android.content.pm.', '').replace('android.content.ComponentName', 'ComponentName')
            target.write_text(STUBS + '\n' + body + CHECKS)
            exception = root / 'android/content/ActivityNotFoundException.java'
            exception.parent.mkdir(parents=True)
            exception.write_text('package android.content;\n'
                                 'public class ActivityNotFoundException extends RuntimeException {\n'
                                 'public ActivityNotFoundException(String message) { super(message); } }\n')
            subprocess.run(['javac', '-d', str(root / 'classes'), str(exception), str(target)], check=True)
            subprocess.run(['java', '-ea', '-cp', str(root / 'classes'), 'DispatchTest'], check=True)


if __name__ == '__main__':
    unittest.main()
