// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
package io.hopea.hopea;

/** In-memory foreground state only; never stores a usage history. */
final class GameActivityState {
    private static final long WINDOW = 5 * 60_000L;
    private long lastQuery = -1;
    private String app, activity;

    long begin(long now) {
        if (now < 0) { reset(); return 0; }
        if (lastQuery < 0 || now < lastQuery || now - lastQuery > WINDOW) {
            clear();
            lastQuery = now;
            return Math.max(0, now - WINDOW);
        }
        long begin = Math.max(0, lastQuery - 2_000); // tolerate late-delivered events
        lastQuery = now;
        return begin;
    }
    void resume(String pkg, String cls) {
        if (pkg == null || pkg.isEmpty() || pkg.length() > 512 || cls == null || cls.isEmpty() || cls.length() > 1024) {
            clear(); return;
        }
        app = pkg; activity = cls;
    }
    void pause(String pkg, String cls) {
        // A late pause of the old activity must not clear a newly resumed one.
        if (app != null && app.equals(pkg) && activity.equals(cls)) clear();
    }
    void clear() { app = null; activity = null; }
    void reset() { clear(); lastQuery = -1; }
    String foreground() { return app; }
}
