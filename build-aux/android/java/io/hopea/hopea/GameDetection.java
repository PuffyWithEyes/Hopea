// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
package io.hopea.hopea;

import android.app.AppOpsManager;
import android.app.KeyguardManager;
import android.app.usage.UsageEvents;
import android.app.usage.UsageStatsManager;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.PowerManager;
import android.os.Process;
import android.provider.Settings;

/** Android APIs only. No accessibility service, root, network or persisted history. */
public final class GameDetection {
    private static volatile Context context;
    private static final GameActivityState state = new GameActivityState();
    private GameDetection() {}
    public static void initialize(Context activity) { context = activity.getApplicationContext(); }

    /** -2 unavailable; -1 permission needed; 0 inactive; 1 foreground game. */
    @SuppressWarnings("deprecation")
    public static synchronized int status() {
        Context c = context;
        if (c == null) return -2;
        try {
            AppOpsManager ops = c.getSystemService(AppOpsManager.class);
            if (ops == null || ops.unsafeCheckOpNoThrow(AppOpsManager.OPSTR_GET_USAGE_STATS,
                    Process.myUid(), c.getPackageName()) != AppOpsManager.MODE_ALLOWED) {
                state.reset(); return -1;
            }
            PowerManager power = c.getSystemService(PowerManager.class);
            KeyguardManager lock = c.getSystemService(KeyguardManager.class);
            if (power == null || !power.isInteractive() || (lock != null && lock.isKeyguardLocked())) {
                state.reset(); return 0;
            }
            UsageStatsManager usage = c.getSystemService(UsageStatsManager.class);
            if (usage == null) { state.reset(); return -2; }
            long now = System.currentTimeMillis();
            UsageEvents events = usage.queryEvents(state.begin(now), now);
            if (events == null) { state.reset(); return -2; }
            UsageEvents.Event event = new UsageEvents.Event();
            int count = 0;
            while (events.hasNextEvent()) {
                // Refuse a partial replay rather than mistaking an old game
                // launch for the current activity after an event flood.
                if (++count > 2048) { state.reset(); return -2; }
                events.getNextEvent(event);
                switch (event.getEventType()) {
                    case UsageEvents.Event.ACTIVITY_RESUMED:
                        state.resume(event.getPackageName(), event.getClassName()); break;
                    case UsageEvents.Event.ACTIVITY_PAUSED:
                    case UsageEvents.Event.ACTIVITY_STOPPED:
                        state.pause(event.getPackageName(), event.getClassName()); break;
                    case UsageEvents.Event.DEVICE_SHUTDOWN:
                    case UsageEvents.Event.SCREEN_NON_INTERACTIVE:
                    case UsageEvents.Event.KEYGUARD_SHOWN:
                        state.clear(); break;
                    default: break;
                }
            }
            String pkg = state.foreground();
            if (pkg == null || pkg.equals(c.getPackageName())) return 0;
            ApplicationInfo info = c.getPackageManager().getApplicationInfo(pkg, 0);
            return info.category == ApplicationInfo.CATEGORY_GAME
                || (info.flags & ApplicationInfo.FLAG_IS_GAME) != 0 ? 1 : 0;
        } catch (PackageManager.NameNotFoundException hidden) {
            state.clear(); return 0;
        } catch (RuntimeException unavailable) {
            state.reset(); return -2;
        }
    }

    /** Only called in response to the listener pressing the settings row. */
    public static boolean openUsageAccess() {
        Context c = context;
        if (c == null) return false;
        Intent intent = new Intent(Settings.ACTION_USAGE_ACCESS_SETTINGS)
            .setData(Uri.parse("package:" + c.getPackageName()))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try { c.startActivity(intent); return true; }
        catch (RuntimeException noDetailPage) {
            try { c.startActivity(intent.setData(null)); return true; }
            catch (RuntimeException unavailable) { return false; }
        }
    }
}
