#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Run the actual report formatter on a JVM; no device or user data required."""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory() as directory:
    output = Path(directory)
    test = output / 'CrashReportTest.java'
    test.write_text(r'''
package io.hopea.hopea;
public class CrashReportTest {
    public static void main(String[] args) {
        Throwable cause = new IllegalStateException("SECRET_TOKEN_AND_INPUT");
        cause.setStackTrace(new StackTraceElement[] {
            new StackTraceElement("android.view.Input", "restart", "PRIVATE_FILENAME", 42)
        });
        Throwable outer = new RuntimeException("PRIVATE_URL", cause);
        String report = CrashReport.describe(outer);
        assert report.contains("IllegalStateException");
        assert report.contains("android.view.Input.restart:42");
        assert !report.contains("SECRET_TOKEN_AND_INPUT");
        assert !report.contains("PRIVATE_URL");
        assert !report.contains("PRIVATE_FILENAME");
        cause.initCause(outer); // Cyclic exception chains must terminate.
        assert CrashReport.describe(outer).contains("remaining causes omitted");
        Throwable large = new RuntimeException("ignored");
        StackTraceElement[] frames = new StackTraceElement[10000];
        for (int i = 0; i < frames.length; i++)
            frames[i] = new StackTraceElement("C".repeat(1000), "m".repeat(1000), "f", 1);
        large.setStackTrace(frames);
        for (int i = 0; i < 20; i++) {
            Throwable next = new RuntimeException("ignored", large);
            next.setStackTrace(frames);
            large = next;
        }
        report = CrashReport.describe(large);
        assert report.length() < 16100;
        assert report.contains("report truncated");
        System.out.println("Crash report: privacy, cyclic causes and size limits passed");
    }
}
''')
    subprocess.run(['javac', '-d', directory,
                    str(root / 'build-aux/android/java/io/hopea/hopea/CrashReport.java'),
                    str(test)], check=True)
    subprocess.run(['java', '-ea', '-cp', directory, 'io.hopea.hopea.CrashReportTest'], check=True)
