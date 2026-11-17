#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check real patched GTK method bodies with recording JNI/UI boundaries.

Pass a staged GTK source directory. These tests do not emulate an Android device.
Set ANDROID_RUNTIME_CC to an NDK clang to also type-check the JNI bridge for
that architecture (particularly ARMv7, where pointers and Java longs differ).
"""
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile


def method(source, declaration):
    start = source.index(declaration)
    opening = source.index('{', start)
    depth = 1
    end = opening + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


gtk = Path(sys.argv[1])
java = gtk / 'gdk/android/glue/java/org/gtk/android'
ime = (gtk / 'gtk/gtkimcontextandroid.c').read_text()
toplevel = (gtk / 'gdk/android/gdkandroidtoplevel.c').read_text()
ime_pointer = re.search(r'gtk_im_context_android_java_cache\.constructor,\s*([^;]+)\)\);', ime).group(1)
window_pointer = re.search(r'->toplevel\.toplevel_identifier_key,\s*([^;]+)\);', toplevel).group(1)
c_source = r'''
#include <stdint.h>
#include <stdarg.h>
#include <assert.h>
typedef int64_t jlong;
typedef long glong;
#define GPOINTER_TO_SIZE(p) ((uintptr_t)(p))
static void capture(void *env, void *klass, void *method, ...) {
    (void)klass;
    va_list args;
    va_start(args, method);
    jlong value = va_arg(args, jlong);
    unsigned int next = va_arg(args, unsigned int);
    va_end(args);
    assert(value == (jlong)(uintptr_t)env);
    assert(next == 0x55667788U);
}
static void check(void *self) {
    /* A Java (J)V parameter must be a jlong before entering JNI varargs. */
    _Static_assert(_Generic((IME_POINTER), jlong: 1, default: 0), "IME pointer is not jlong");
    _Static_assert(_Generic((WINDOW_POINTER), jlong: 1, default: 0), "Window pointer is not jlong");
    capture(self, 0, 0, IME_POINTER, 0x55667788U);
    capture(self, 0, 0, WINDOW_POINTER, 0x55667788U);
}
int main(void) {
    check((void *)(uintptr_t)0xf1234567U);
#if UINTPTR_MAX > UINT32_MAX
    check((void *)(uintptr_t)0x7fff12345678ULL);
#endif
}
'''.replace('IME_POINTER', ime_pointer).replace('WINDOW_POINTER', window_pointer)

reset = method((java / 'ImContext.java').read_text(), 'private static void reset(')
activity = (java / 'ToplevelActivity.java').read_text()
launch = (method(activity, 'public void launchActivityForResult(')
          if 'public void launchActivityForResult(' in activity else '')
assert '->toplevel.launch_activity_for_result,' in toplevel
cache = (gtk / 'gdk/android/gdkandroidinit.c').read_text()
assert 'launch_activity_for_result, "launchActivityForResult", "(Landroid/content/Intent;I)V"' in cache
result = method(activity, 'protected void onActivityResult(')
keys = method(activity, 'private boolean keyEventProxy(')
assert 'setVolumeControlStream(android.media.AudioManager.STREAM_MUSIC)' in activity
java_source = r'''
import java.util.ArrayDeque;
import java.util.logging.Logger;
@interface Nullable {}
class Ui {
    static boolean active;
    static ArrayDeque<Runnable> pending = new ArrayDeque<>();
    static void drain() {
        active = true;
        try { while (!pending.isEmpty()) pending.remove().run(); }
        finally { active = false; }
    }
}
class GlibContext {
    static ArrayDeque<Runnable> pending = new ArrayDeque<>();
    static void runOnMain(Runnable task) { pending.add(task); }
    static void drain() { while (!pending.isEmpty()) pending.remove().run(); }
}
class Intent {}
class Bundle {}
class InputMethodManager {
    int resets;
    void restartInput(View view) { assert Ui.active : "IME reset outside UI thread"; resets++; }
}
class Context {
    InputMethodManager imm = new InputMethodManager();
    <T> T getSystemService(Class<T> type) { return type.cast(imm); }
}
class View {
    Context context = new Context();
    Context getContext() { return context; }
    boolean post(Runnable task) { Ui.pending.add(task); return true; }
}
class ImContext {
    RESET
    static void exercise(View view) { reset(view); }
}
class Activity {
    static final int RESULT_CANCELED = 0;
    int launches, responses, lastCode, lastResult;
    Intent lastIntent, responseIntent;
    Bundle lastOptions;
    RuntimeException failure;
    void runOnUiThread(Runnable task) { if (Ui.active) task.run(); else Ui.pending.add(task); }
    public void startActivityForResult(Intent intent, int code) { startActivityForResult(intent, code, null); }
    public void startActivityForResult(Intent intent, int code, Bundle options) {
        assert Ui.active : "Activity launch outside UI thread";
        launches++; lastIntent = intent; lastCode = code; lastOptions = options;
        if (failure != null) throw failure;
    }
    protected void onActivityResult(int code, int result, Intent data) {}
    void notifyActivityResult(int code, int result, Intent data) {
        assert !Ui.active : "GTK callback on Android UI thread";
        responses++; lastCode = code; lastResult = result; responseIntent = data;
    }
}
class ToplevelActivity extends Activity {
    LAUNCH
    RESULT
}
class RuntimeTest {
    public static void main(String[] args) {
        Surface surface = new Surface();
        for (int key : new int[] {4, 24, 25, 164}) {
            assert !surface.send(key);
            assert !surface.send(key);
        }
        assert GlibContext.pending.isEmpty();
        assert surface.send(29);
        assert surface.forwarded == 0;
        GlibContext.drain();
        assert surface.forwarded == 1;
        View view = new View();
        ImContext.exercise(view);
        assert view.context.imm.resets == 0;
        Ui.drain();
        assert view.context.imm.resets == 1;
        view.context.imm = null;
        ImContext.exercise(view); Ui.drain(); // missing IME service is harmless
        for (int mode = 0; mode < 3; mode++) {
            for (int code : new int[] {17, -1}) {
                ToplevelActivity activity = new ToplevelActivity();
                Intent intent = new Intent();
                if (mode == 1) activity.failure = new android.content.ActivityNotFoundException();
                if (mode == 2) activity.failure = new SecurityException();
                activity.launchActivityForResult(intent, code); // JNI's dedicated entry
                assert activity.launches == 0 && activity.responses == 0;
                Ui.drain();
                assert activity.launches == 1 && activity.lastIntent == intent && activity.lastCode == code;
                assert activity.responses == 0;
                GlibContext.drain();
                assert activity.responses == (mode != 0 && code >= 0 ? 1 : 0);
                if (activity.responses != 0) assert activity.lastResult == Activity.RESULT_CANCELED && activity.responseIntent == null;
            }
        }
        ToplevelActivity activity = new ToplevelActivity();
        Bundle options = new Bundle();
        Intent intent = new Intent();
        Ui.active = true;
        activity.startActivityForResult(intent, 31, options);
        Ui.active = false;
        assert activity.launches == 1 && activity.lastOptions == options;
        // Ordinary launches must still throw synchronously for client/browser fallback.
        activity.failure = new android.content.ActivityNotFoundException();
        Ui.active = true;
        boolean threw = false;
        try { activity.startActivityForResult(intent, -1); }
        catch (android.content.ActivityNotFoundException expected) { threw = true; }
        finally { Ui.active = false; }
        assert threw && Ui.pending.isEmpty();
        activity.onActivityResult(31, -1, intent);
        GlibContext.drain();
        assert activity.responses == 1 && activity.lastResult == -1 && activity.responseIntent == intent;
        assert Ui.pending.isEmpty() && GlibContext.pending.isEmpty();
    }
}
'''.replace('RESET', reset).replace('LAUNCH', launch).replace('RESULT\n', result + '\n')
java_source += r'''
class KeyEvent {
    static final int KEYCODE_BACK = 4, KEYCODE_VOLUME_UP = 24,
        KEYCODE_VOLUME_DOWN = 25, KEYCODE_VOLUME_MUTE = 164;
    int code;
    KeyEvent(int code) { this.code = code; }
    int getKeyCode() { return code; }
}
class Surface {
    int forwarded;
    KEYS
    void notifyKeyEvent(KeyEvent event) { forwarded++; }
    boolean send(int code) { return keyEventProxy(new KeyEvent(code)); }
}
'''.replace('KEYS', keys)

with tempfile.TemporaryDirectory(prefix='hopea-android-runtime-') as directory:
    root = Path(directory)
    source = root / 'jni.c'
    source.write_text(c_source)
    cc = shlex.split(os.environ.get('CC', 'cc'))
    subprocess.run([*cc, '-std=c11', '-Wall', '-Wextra', str(source), '-o', str(root / 'jni')], check=True)
    subprocess.run([str(root / 'jni')], check=True)
    if os.environ.get('ANDROID_RUNTIME_CC'):
        subprocess.run([*shlex.split(os.environ['ANDROID_RUNTIME_CC']), '-std=c11', '-c', str(source), '-o', str(root / 'jni.o')], check=True)
    (root / 'RuntimeTest.java').write_text(java_source)
    exception = root / 'android/content/ActivityNotFoundException.java'
    exception.parent.mkdir(parents=True)
    exception.write_text('package android.content; public class ActivityNotFoundException extends RuntimeException {}\n')
    subprocess.run(['javac', '-d', str(root), str(exception), str(root / 'RuntimeTest.java')], check=True)
    subprocess.run(['java', '-ea', '-cp', str(root), 'RuntimeTest'], check=True)
print('Android JNI widths, IME/activity dispatch and system volume-key routing: passed')
