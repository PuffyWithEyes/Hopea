// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
package io.hopea.hopea;

import android.app.Activity;
import android.graphics.Rect;
import android.os.Build;
import android.view.View;
import android.view.ViewTreeObserver;
import android.view.WindowInsets;
import java.lang.ref.WeakReference;

/** Read Android views on its UI thread; GTK only reads the published snapshot. */
public final class KeyboardVisibility {
    private static volatile int visible = -1;
    private static WeakReference<Activity> owner = new WeakReference<>(null);
    private static View decor;
    private static ViewTreeObserver.OnGlobalLayoutListener listener;

    private KeyboardVisibility() { }

    public static int current() { return visible; }

    public static void watch(Activity activity) {
        activity.runOnUiThread(() -> {
            if (activity.isFinishing() || activity.isDestroyed()) return;
            if (owner.get() == activity && decor != null) {
                update();
                return;
            }
            detach();
            owner = new WeakReference<>(activity);
            decor = activity.getWindow().getDecorView();
            listener = KeyboardVisibility::update;
            decor.getViewTreeObserver().addOnGlobalLayoutListener(listener);
            update();
        });
    }

    public static void stop(Activity activity) {
        activity.runOnUiThread(() -> {
            if (owner.get() == activity) detach();
        });
    }

    private static void detach() {
        if (decor != null && listener != null && decor.getViewTreeObserver().isAlive())
            decor.getViewTreeObserver().removeOnGlobalLayoutListener(listener);
        decor = null;
        listener = null;
        owner.clear();
        visible = -1;
    }

    private static void update() {
        if (decor == null) return;
        if (Build.VERSION.SDK_INT >= 30) {
            WindowInsets insets = decor.getRootWindowInsets();
            if (insets != null)
                visible = insets.isVisible(WindowInsets.Type.ime()) ? 1 : 0;
        } else {
            // Android 9/10 lack IME insets. Ignore ordinary navigation/status
            // bars while detecting the area occupied by the software keyboard.
            Rect frame = new Rect();
            decor.getWindowVisibleDisplayFrame(frame);
            int height = decor.getRootView().getHeight();
            if (height <= 0) return;
            int threshold = Math.max((int)(100 * decor.getResources().getDisplayMetrics().density), height / 6);
            visible = height - frame.bottom > threshold ? 1 : 0;
        }
    }
}
