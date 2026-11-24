// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
package io.hopea.hopea;

import android.content.*;
import android.os.*;
import java.io.IOException;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;

/** Private FD transport. Binder intents contain no tokens, cookies or URLs. */
public final class ImportBridge {
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static final AtomicInteger NEXT = new AtomicInteger(1);
    private static final ConcurrentHashMap<Integer, Job> JOBS = new ConcurrentHashMap<>();
    private static Context context;
    public static void initialize(Context activity) { context = activity.getApplicationContext(); }

    public static synchronized int start(int fd, int mode) {
        if (context == null || !JOBS.isEmpty() || mode < 0 || mode > 2) return -1;
        try {
            int id = NEXT.getAndIncrement();
            Job job = new Job(id, ParcelFileDescriptor.fromFd(fd), mode);
            JOBS.put(id, job);
            MAIN.post(job::start);
            return id;
        } catch (IOException error) { return -1; }
    }
    public static int running(int id) { return JOBS.containsKey(id) ? 1 : 0; }
    public static void cancel(int id) {
        MAIN.post(() -> { Job job = JOBS.get(id); if (job != null) job.close(); });
    }
    static ParcelFileDescriptor takeLoginPipe(int id) {
        Job job = JOBS.get(id);
        if (job == null || job.mode != 2) return null;
        ParcelFileDescriptor pipe = job.pipe; job.pipe = null;
        return pipe;
    }
    static void loginFinished(int id) {
        Job job = JOBS.get(id); if (job != null) job.close();
    }
    private static final class Job implements ServiceConnection {
        final int id, mode;
        ParcelFileDescriptor pipe;
        Messenger remote;
        boolean bound, closed, watched;
        Job(int id, ParcelFileDescriptor pipe, int mode) {
            this.id = id; this.pipe = pipe; this.mode = mode;
        }
        void start() {
            if (closed) return;
            try {
                if (mode == 2) {
                    context.startActivity(new Intent(context, VkSignInActivity.class)
                        .putExtra("job", id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
                } else {
                    bound = context.bindService(new Intent(context, ImportService.class), this, Context.BIND_AUTO_CREATE);
                    if (!bound) close();
                }
                // Binding/startup must not hang forever without producing an event.
                MAIN.postDelayed(() -> { if (!closed && pipe != null) close(); }, 30000);
            } catch (RuntimeException unavailable) { close(); }
        }
        @Override public void onServiceConnected(ComponentName name, IBinder binder) {
            if (closed) return;
            remote = new Messenger(binder);
            try {
                binder.linkToDeath(() -> MAIN.post(() -> { watched = false; close(); JOBS.remove(id); }), 0);
                watched = true;
                Message request = Message.obtain(null, 1);
                Bundle data = new Bundle(); data.putParcelable("pipe", pipe); data.putInt("mode", mode);
                request.setData(data); remote.send(request);
                pipe.close(); pipe = null;
            } catch (Exception failure) { close(); }
        }
        @Override public void onServiceDisconnected(ComponentName name) { close(); }
        @Override public void onBindingDied(ComponentName name) { close(); }
        @Override public void onNullBinding(ComponentName name) { close(); }
        void close() {
            if (closed) return;
            closed = true;
            if (remote != null) {
                try { remote.send(Message.obtain(null, 2)); } catch (RemoteException ignored) { }
            }
            if (pipe != null) { try { pipe.close(); } catch (IOException ignored) { } pipe = null; }
            if (bound) { context.unbindService(this); bound = false; }
            if (!watched) JOBS.remove(id);
        }
    }
}
