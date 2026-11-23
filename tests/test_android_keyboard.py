#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise the production keyboard observer at Android's UI-thread boundary."""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
sources = {
    'android/os/Build.java': '''package android.os;
public class Build { public static class VERSION { public static int SDK_INT = 34; } }''',
    'android/graphics/Rect.java': '''package android.graphics;
public class Rect { public int bottom; }''',
    'android/util/DisplayMetrics.java': '''package android.util;
public class DisplayMetrics { public float density = 1; }''',
    'android/content/res/Resources.java': '''package android.content.res;
public class Resources {
  public android.util.DisplayMetrics getDisplayMetrics() { return new android.util.DisplayMetrics(); }
}''',
    'android/view/WindowInsets.java': '''package android.view;
public class WindowInsets {
  public boolean shown;
  public boolean isVisible(int type) { return shown; }
  public static class Type { public static int ime() { return 8; } }
}''',
    'android/view/ViewTreeObserver.java': '''package android.view;
public class ViewTreeObserver {
  public interface OnGlobalLayoutListener { void onGlobalLayout(); }
  public OnGlobalLayoutListener listener;
  public int adds, removes;
  public boolean isAlive() { return true; }
  public void addOnGlobalLayoutListener(OnGlobalLayoutListener value) { listener = value; adds++; }
  public void removeOnGlobalLayoutListener(OnGlobalLayoutListener value) {
    assert listener == value; listener = null; removes++;
  }
  public void dispatch() {
    android.app.Activity.ui = true;
    try { if (listener != null) listener.onGlobalLayout(); }
    finally { android.app.Activity.ui = false; }
  }
}''',
    'android/view/View.java': '''package android.view;
public class View {
  public WindowInsets insets = new WindowInsets();
  public final ViewTreeObserver observer = new ViewTreeObserver();
  public int height = 800, bottom = 780;
  public ViewTreeObserver getViewTreeObserver() { assert android.app.Activity.ui; return observer; }
  public WindowInsets getRootWindowInsets() { assert android.app.Activity.ui; return insets; }
  public void getWindowVisibleDisplayFrame(android.graphics.Rect frame) {
    assert android.app.Activity.ui; frame.bottom = bottom;
  }
  public View getRootView() { return this; }
  public int getHeight() { return height; }
  public android.content.res.Resources getResources() { return new android.content.res.Resources(); }
}''',
    'android/view/Window.java': '''package android.view;
public class Window {
  public final View view = new View();
  public View getDecorView() { assert android.app.Activity.ui; return view; }
}''',
    'android/app/Activity.java': '''package android.app;
public class Activity {
  public static boolean ui;
  private static final java.util.ArrayDeque<Runnable> pending = new java.util.ArrayDeque<>();
  public final android.view.Window window = new android.view.Window();
  public boolean finished;
  public boolean isFinishing() { return finished; }
  public boolean isDestroyed() { return false; }
  public android.view.Window getWindow() { return window; }
  public void runOnUiThread(Runnable task) { pending.add(task); }
  public static void drain() {
    ui = true;
    try { while (!pending.isEmpty()) pending.remove().run(); }
    finally { ui = false; }
  }
}''',
    'io/hopea/hopea/KeyboardVisibilityTest.java': '''package io.hopea.hopea;
import android.app.Activity;
import android.os.Build;
public class KeyboardVisibilityTest {
  public static void main(String[] args) {
    assert KeyboardVisibility.current() == -1;
    Activity first = new Activity();
    KeyboardVisibility.watch(first);
    assert KeyboardVisibility.current() == -1; // no views touched on GTK's thread
    Activity.drain();
    assert KeyboardVisibility.current() == 0;
    first.window.view.insets.shown = true;
    first.window.view.observer.dispatch();
    assert KeyboardVisibility.current() == 1;
    // Back hides IME without changing the text entry's focus.
    first.window.view.insets.shown = false;
    first.window.view.observer.dispatch();
    assert KeyboardVisibility.current() == 0;
    KeyboardVisibility.watch(first); Activity.drain();
    assert first.window.view.observer.adds == 1;
    Activity second = new Activity();
    second.window.view.insets = null;
    KeyboardVisibility.watch(second); Activity.drain();
    assert first.window.view.observer.removes == 1;
    assert KeyboardVisibility.current() == -1; // insets not ready yet
    KeyboardVisibility.stop(first); Activity.drain();
    assert second.window.view.observer.listener != null;
    KeyboardVisibility.stop(second); Activity.drain();
    assert second.window.view.observer.removes == 1;
    Build.VERSION.SDK_INT = 28;
    KeyboardVisibility.watch(first); Activity.drain();
    assert KeyboardVisibility.current() == 0; // navigation bar is not a keyboard
    first.window.view.bottom = 480;
    first.window.view.observer.dispatch();
    assert KeyboardVisibility.current() == 1;
    first.window.view.bottom = 780;
    first.window.view.observer.dispatch();
    assert KeyboardVisibility.current() == 0;
    KeyboardVisibility.stop(first); Activity.drain();
    assert KeyboardVisibility.current() == -1;
    first.finished = true;
    KeyboardVisibility.watch(first); Activity.drain();
    assert first.window.view.observer.listener == null;
    System.out.println("Android keyboard: UI-thread reads, show/hide, lifecycle and API 28 fallback passed");
  }
}''',
}
with tempfile.TemporaryDirectory() as directory:
    work = Path(directory)
    files = []
    for name, source in sources.items():
        path = work / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)
        files.append(str(path))
    files.append(str(root / 'build-aux/android/java/io/hopea/hopea/KeyboardVisibility.java'))
    subprocess.run(['javac', '-d', str(work / 'classes'), *files], check=True)
    subprocess.run(['java', '-ea', '-cp', str(work / 'classes'),
                    'io.hopea.hopea.KeyboardVisibilityTest'], check=True)
