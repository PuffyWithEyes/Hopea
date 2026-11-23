// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
package io.hopea.hopea;

import android.app.Activity;
import android.app.AlertDialog;
import android.app.Application;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.os.Build;
import android.os.Bundle;
import android.util.AtomicFile;

import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.atomic.AtomicBoolean;

/** Local Java crash diagnostics, including on Android 9/10 without ADB. */
public final class HopeaApplication extends org.gtk.android.RuntimeApplication {
    private AtomicFile crashFile;
    private String previousReport;
    private final AtomicBoolean reporting = new AtomicBoolean();

    @Override
    public void onCreate() {
        // Resources includes Android's per-app language override. Export it
        // before super.onCreate starts GTK; Android has no desktop LANG setup.
        prepareLanguages();
        // GTK refreshes filesDir when runtime assets change. Keep diagnostics
        // separately, private to this app and excluded from Android backups.
        crashFile = new AtomicFile(new File(getNoBackupFilesDir(), "java-crash.txt"));
        try {
            if (crashFile.getBaseFile().length() <= 128 * 1024)
                previousReport = new String(crashFile.readFully(), StandardCharsets.UTF_8);
            else
                crashFile.delete();
        } catch (java.io.IOException absent) {
            previousReport = null;
        }

        String version;
        try {
            version = getPackageManager().getPackageInfo(getPackageName(), 0).versionName;
        } catch (android.content.pm.PackageManager.NameNotFoundException unavailable) {
            version = "unknown";
        }
        final String header = "Hopea " + version + "\nAndroid API " + Build.VERSION.SDK_INT
            + "\nDevice: " + Build.MANUFACTURER + " " + Build.MODEL
            + "\nProcess: " + (android.os.Process.is64Bit() ? "64-bit" : "32-bit")
            + "\nJava exception (messages omitted):\n";
        Thread.UncaughtExceptionHandler previous = Thread.getDefaultUncaughtExceptionHandler();
        Thread.setDefaultUncaughtExceptionHandler((thread, failure) -> {
            if (reporting.compareAndSet(false, true)) {
                FileOutputStream output = null;
                try {
                    output = crashFile.startWrite();
                    output.write((header + CrashReport.describe(failure)).getBytes(StandardCharsets.UTF_8));
                    crashFile.finishWrite(output);
                } catch (Throwable cannotWrite) {
                    // Reporting must not replace the original crash handling.
                    try { crashFile.failWrite(output); } catch (Throwable ignored) { }
                }
            }
            if (previous != null) {
                previous.uncaughtException(thread, failure);
            } else {
                android.os.Process.killProcess(android.os.Process.myPid());
            }
        });

        registerActivityLifecycleCallbacks(new Application.ActivityLifecycleCallbacks() {
            @Override public void onActivityResumed(Activity activity) {
                KeyboardVisibility.watch(activity);
                if (previousReport == null || activity.isFinishing()) return;
                String report = previousReport;
                previousReport = null;
                new AlertDialog.Builder(activity)
                    .setTitle("Previous Hopea crash")
                    .setMessage("Hopea saved a technical report on this device. It contains the app version, "
                        + "device model and Java stack trace, without exception messages or entered text. "
                        + "Nothing is sent automatically. You can copy it and send it with your bug report.")
                    .setPositiveButton("Copy report", (dialog, which) -> {
                        ClipboardManager clipboard = getSystemService(ClipboardManager.class);
                        if (clipboard != null)
                            clipboard.setPrimaryClip(ClipData.newPlainText("Hopea crash report", report));
                    })
                    .setNeutralButton("Later", (dialog, which) -> { })
                    .setNegativeButton("Delete report", (dialog, which) -> crashFile.delete())
                    .show();
            }
            @Override public void onActivityCreated(Activity activity, Bundle state) { }
            @Override public void onActivityStarted(Activity activity) { }
            @Override public void onActivityPaused(Activity activity) { KeyboardVisibility.stop(activity); }
            @Override public void onActivityStopped(Activity activity) { }
            @Override public void onActivitySaveInstanceState(Activity activity, Bundle state) { }
            @Override public void onActivityDestroyed(Activity activity) { KeyboardVisibility.stop(activity); }
        });
        super.onCreate();
    }

    private void prepareLanguages() {
        try {
            android.system.Os.setenv("HOPEA_SYSTEM_LANGUAGES",
                getResources().getConfiguration().getLocales().toLanguageTags(), true);
            android.system.Os.setenv("HOPEA_LOCALE_DIR",
                new File(getFilesDir(), "share/locale").getAbsolutePath(), true);
        } catch (android.system.ErrnoException unavailable) {
            // Native startup falls back to English if detection is unavailable.
            android.util.Log.w("Hopea", "Unable to initialize system language");
        }
    }
}
