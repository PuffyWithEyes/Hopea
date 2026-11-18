#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise the startup language bridge with recording Android API boundaries."""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
source = (root / "build-aux/android/java/io/hopea/hopea/HopeaApplication.java").read_text()
start = source.index("private void prepareLanguages()")
opening = source.index("{", start)
end, depth = opening + 1, 1
while depth:
    depth += (source[end] == "{") - (source[end] == "}")
    end += 1
method = source[start:end]
assert source.index("prepareLanguages();") < source.index("        super.onCreate();")
test = r'''
import java.io.File;
class android {
    static class system {
        static class ErrnoException extends Exception {}
        static class Os {
            static boolean fail;
            static java.util.Map<String, String> values = new java.util.HashMap<>();
            static void setenv(String key, String value, boolean overwrite) throws ErrnoException {
                if (fail) throw new ErrnoException();
                assert overwrite;
                values.put(key, value);
            }
        }
    }
    static class util { static class Log { static void w(String tag, String text) {} } }
}
public class LanguageTest {
    String languages = "ru-RU,en-US";
    LanguageTest getResources() { return this; }
    LanguageTest getConfiguration() { return this; }
    LanguageTest getLocales() { return this; }
    String toLanguageTags() { return languages; }
    File getFilesDir() { return new File("/private/hopea"); }
    METHOD
    public static void main(String[] args) {
        LanguageTest app = new LanguageTest();
        app.prepareLanguages();
        assert android.system.Os.values.get("HOPEA_SYSTEM_LANGUAGES").equals("ru-RU,en-US");
        assert android.system.Os.values.get("HOPEA_LOCALE_DIR").equals("/private/hopea/share/locale");
        app.languages = "zh-Hant-TW";
        app.prepareLanguages();
        assert android.system.Os.values.get("HOPEA_SYSTEM_LANGUAGES").equals("zh-Hant-TW");
        android.system.Os.values.clear();
        android.system.Os.fail = true;
        app.prepareLanguages(); // Unavailable detection must not abort startup.
        assert android.system.Os.values.isEmpty();
        System.out.println("Android language bridge: preferred locales, asset path and failure passed");
    }
}
'''.replace("METHOD", method)
with tempfile.TemporaryDirectory() as directory:
    path = Path(directory) / "LanguageTest.java"
    path.write_text(test)
    subprocess.run(["javac", "-d", directory, str(path)], check=True)
    subprocess.run(["java", "-ea", "-cp", directory, "LanguageTest"], check=True)
