// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
package io.hopea.hopea;

import android.app.Service;
import android.content.Intent;
import android.os.*;
import com.chaquo.python.Python;
import com.chaquo.python.android.AndroidPlatform;

/** One disposable Python worker, in :import, never in GTK's process. */
public final class ImportService extends Service {
    public static byte[] decryptAesCbc(byte[] data, byte[] key, byte[] iv) throws java.security.GeneralSecurityException {
        javax.crypto.Cipher cipher = javax.crypto.Cipher.getInstance("AES/CBC/NoPadding");
        cipher.init(javax.crypto.Cipher.DECRYPT_MODE, new javax.crypto.spec.SecretKeySpec(key, "AES"),
            new javax.crypto.spec.IvParameterSpec(iv));
        return cipher.doFinal(data);
    }
    private boolean started;
    private final Messenger messenger = new Messenger(new Handler(Looper.getMainLooper(), message -> {
        if (message.what == 2) {
            android.os.Process.killProcess(android.os.Process.myPid());
            return true;
        }
        if (message.what != 1 || started) return true;
        started = true;
        Bundle data = message.getData();
        data.setClassLoader(ParcelFileDescriptor.class.getClassLoader());
        ParcelFileDescriptor pipe = data.getParcelable("pipe");
        int mode = data.getInt("mode", -1);
        if (pipe == null || mode < 0 || mode > 1) {
            android.os.Process.killProcess(android.os.Process.myPid());
            return true;
        }
        new Thread(() -> {
            try {
                Python.start(new AndroidPlatform(this));
                Python.getInstance().getModule("android_import").callAttr("run", pipe.detachFd(), mode);
            } catch (Throwable failure) {
                // No exception messages: provider errors can contain credentials.
                try { pipe.close(); } catch (Exception ignored) { }
            } finally {
                // Python, cookies, sockets and any stuck provider threads die together.
                android.os.Process.killProcess(android.os.Process.myPid());
            }
        }, "Hopea service adapter").start();
        return true;
    }));
    @Override public IBinder onBind(Intent intent) { return messenger.getBinder(); }
    @Override public boolean onUnbind(Intent intent) {
        android.os.Process.killProcess(android.os.Process.myPid());
        return false;
    }
}
