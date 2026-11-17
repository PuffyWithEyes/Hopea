// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
package io.hopea.hopea;

import java.util.Collections;
import java.util.IdentityHashMap;
import java.util.Set;

/** A bounded Java stack trace without exception messages or user input. */
final class CrashReport {
    private CrashReport() {}

    static String describe(Throwable failure) {
        StringBuilder report = new StringBuilder();
        Set<Throwable> seen = Collections.newSetFromMap(new IdentityHashMap<>());
        for (int cause = 0; failure != null && cause < 8 && seen.add(failure); cause++) {
            if (cause != 0) report.append("Caused by: ");
            report.append(label(failure.getClass().getName())).append('\n');
            StackTraceElement[] frames = failure.getStackTrace();
            for (int index = 0; index < Math.min(frames.length, 32); index++) {
                StackTraceElement frame = frames[index];
                report.append("  at ").append(label(frame.getClassName()))
                    .append('.').append(label(frame.getMethodName()))
                    .append(':').append(frame.getLineNumber()).append('\n');
            }
            if (frames.length > 32) report.append("  [remaining frames omitted]\n");
            failure = failure.getCause();
        }
        if (failure != null) report.append("[remaining causes omitted]\n");
        if (report.length() > 16000) {
            report.setLength(16000);
            report.append("\n[report truncated]\n");
        }
        return report.toString();
    }

    private static String label(String value) {
        return value.substring(0, Math.min(value.length(), 160));
    }
}
