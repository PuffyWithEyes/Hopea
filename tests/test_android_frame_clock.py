#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise the patched GTK callback with a recording scheduler and clock.

The native APK build separately checks the real ARMv7 callback ABI. This test
simulates truncated legacy timestamps and long uptimes without a phone.
"""
from pathlib import Path
import subprocess
import sys
import tempfile


def function(source, name):
    start = source.index(name + ' (')
    opening = source.index('{', start)
    depth = 1
    end = opening + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


source = Path(sys.argv[1]).read_text()
callback = function(source, 'gdk_android_choreographer_source_on_frame')
getter = function(source, 'gdk_android_choreographer_source_get_presentation_time')
preamble = r'''
#include <assert.h>
#include <stdint.h>
#include <stdatomic.h>
#include <stddef.h>
#if LEGACY32
#undef __LP64__
#endif
#define FALSE 0
#define TRUE 1
#define G_GINT64_CONSTANT(value) INT64_C(value)
#define g_return_val_if_fail(condition, value) if (!(condition)) return (value)
#define g_atomic_int_get(ptr) atomic_load(ptr)
static int compare(atomic_int *ptr, int before, int after) {
    return atomic_compare_exchange_strong(ptr, &before, after);
}
#define g_atomic_int_compare_and_exchange compare
static int wakeups, posts;
static int64_t clock_us;
static int64_t g_get_monotonic_time(void) { return clock_us; }
static void g_main_context_wakeup(void *context) { assert(context); ++wakeups; }
#if __ANDROID_API__ < 29
typedef long GdkAndroidFrameTime;
#else
typedef int64_t GdkAndroidFrameTime;
#endif
static void AChoreographer_postFrameCallback64(void *choreographer,
        void (*callback)(GdkAndroidFrameTime, void *), void *data) {
    assert(choreographer && callback && data); ++posts;
}
typedef struct {
    atomic_int paused, needs_dispatch;
    atomic_uint_least64_t presentation_time;
    void *context, *choreographer;
} GdkAndroidChoreographerSource;
'''
checks = r'''
int main(void) {
    GdkAndroidChoreographerSource self = {0};
    self.context = self.choreographer = &self;
    atomic_init(&self.presentation_time, 0);
    /* The legacy signed timestamp wraps every ~2.1 seconds, while uptime
     * and animation time must continue advancing for weeks. */
    const int64_t times[] = {2147483000LL, 2147485000LL, 4294968000LL,
                            604800000000000LL, 604800016666000LL};
    for (unsigned i = 0; i < sizeof(times) / sizeof(times[0]); ++i) {
        clock_us = times[i] / 1000;
        int64_t supplied = times[i] - 1000000; /* callback delivery latency */
        int legacy_clock = LEGACY32 && __ANDROID_API__ < 29;
        GdkAndroidFrameTime input = legacy_clock ? (int32_t)supplied : supplied;
        atomic_store(&self.needs_dispatch, FALSE);
        gdk_android_choreographer_source_on_frame(input, &self);
        int64_t expected = legacy_clock ? clock_us * 1000 : supplied;
        assert(gdk_android_choreographer_source_get_presentation_time(&self) == expected);
        assert(wakeups == (int)i + 1 && posts == (int)i + 1);
    }
    int previous_wakeups = wakeups;
    gdk_android_choreographer_source_on_frame(0, &self);
    assert(wakeups == previous_wakeups); /* coalesce pending frames */
    int previous_posts = posts;
    uint64_t previous_time = atomic_load(&self.presentation_time);
    atomic_store(&self.paused, TRUE);
    gdk_android_choreographer_source_on_frame(0, &self);
    gdk_android_choreographer_source_on_frame(0, NULL);
    assert(posts == previous_posts && wakeups == previous_wakeups);
    assert(atomic_load(&self.presentation_time) == previous_time);
}
'''
with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    test = root / 'clock.c'
    test.write_text(preamble + '\nstatic void\n' + callback + '\nstatic int64_t\n' + getter + checks)
    for api, legacy32 in [(28, 1), (28, 0), (29, 1), (29, 0)]:
        binary = root / f'clock-{api}-{legacy32}'
        subprocess.run(['cc', '-std=c11', '-Wall', '-Werror', '-Wno-unused-function',
                        f'-D__ANDROID_API__={api}', f'-DLEGACY32={legacy32}',
                        str(test), '-o', str(binary)], check=True)
        subprocess.run([str(binary)], check=True)
print('Android frame clock: rollover, long uptime, pause and coalescing passed')
