// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
package io.hopea.hopea;

import android.app.*;
import android.content.*;
import android.content.pm.ServiceInfo;
import android.media.*;
import android.media.session.*;
import android.net.*;
import android.os.*;
import java.net.InetAddress;

/** System transport only: all sound is still produced by Hopea's single player. */
public final class PlaybackService extends Service {
    private static final String CHANNEL = "hopea-playback";
    private static final int NOTIFICATION = 41;
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static Context context;
    private static PlaybackService instance;
    private static boolean playing;
    private static String title = "", artist = "";
    private static long duration, position;
    private MediaSession session;
    private AudioManager audio;
    private AudioFocusRequest focus;
    private boolean focused, foreground;
    private PowerManager.WakeLock wake;
    private final BroadcastReceiver noisy = new BroadcastReceiver() {
        @Override public void onReceive(Context c, Intent i) { action(0); }
    };
    private static native void nativeAction(int command);
    private static void action(int command) {
        try { nativeAction(command); }
        catch (UnsatisfiedLinkError starting) {
            // An old notification can start a new process before GTK registered
            // its bridge. Never crash or play a different track in this case.
            if (command == 1 && context != null) {
                Intent open = context.getPackageManager().getLaunchIntentForPackage(context.getPackageName());
                if (open != null) context.startActivity(open.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            }
        }
    }

    public static void initialize(Context activity) {
        context = activity.getApplicationContext();
        GameDetection.initialize(activity);
    }
    public static int gameStatus() { return GameDetection.status(); }
    public static boolean openUsageAccess() { return GameDetection.openUsageAccess(); }
    public static void update(boolean active, String song, String performer, long length, long elapsed) {
        MAIN.post(() -> {
            playing = active; title = song; artist = performer;
            duration = Math.max(0, length); position = Math.max(0, elapsed);
            if (instance != null) instance.refresh();
            else if (active && context != null) {
                try { context.startForegroundService(new Intent(context, PlaybackService.class)); }
                catch (RuntimeException denied) { action(0); }
            }
        });
    }
    @Override public void onCreate() {
        super.onCreate(); instance = this; context = getApplicationContext();
        getSystemService(NotificationManager.class).createNotificationChannel(
            new NotificationChannel(CHANNEL, "Music playback", NotificationManager.IMPORTANCE_LOW));
        session = new MediaSession(this, "Hopea");
        session.setPlaybackToLocal(new AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC).build());
        session.setCallback(new MediaSession.Callback() {
            @Override public void onPlay() { resume(); }
            @Override public void onPause() { action(0); }
            @Override public void onSkipToPrevious() { action(2); }
            @Override public void onSkipToNext() { action(3); }
            @Override public void onStop() { action(4); stopSelf(); }
        }, MAIN);
        session.setActive(true);
        audio = getSystemService(AudioManager.class);
        focus = new AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(new AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC).build())
            .setOnAudioFocusChangeListener(change -> {
                if (change < 0) { focused = false; action(0); }
            }, MAIN).build();
        wake = getSystemService(PowerManager.class).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Hopea:audio");
        wake.setReferenceCounted(false);
        if (Build.VERSION.SDK_INT >= 33) registerReceiver(noisy, new IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY), Context.RECEIVER_NOT_EXPORTED);
        else registerReceiver(noisy, new IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY));
    }
    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        // startForeground must occur even if Pause arrived while starting.
        promote();
        if (intent != null && intent.hasExtra("command")) {
            int command = intent.getIntExtra("command", 0);
            if (command == 1) resume(); else action(command);
        }
        refresh();
        return START_NOT_STICKY; // Never unexpectedly restart music after a kill.
    }
    private void promote() {
        if (Build.VERSION.SDK_INT >= 29)
            startForeground(NOTIFICATION, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
        else
            startForeground(NOTIFICATION, notification());
        foreground = true;
    }
    private void resume() {
        try { promote(); action(1); } catch (RuntimeException denied) { action(0); }
    }
    private PendingIntent command(int command) {
        Intent intent = new Intent(this, PlaybackService.class).putExtra("command", command);
        return PendingIntent.getForegroundService(this, command, intent, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }
    private Notification notification() {
        Intent open = getPackageManager().getLaunchIntentForPackage(getPackageName());
        Notification.Builder builder = new Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_media_play).setContentTitle(title).setContentText(artist)
            .setOnlyAlertOnce(true).setVisibility(Notification.VISIBILITY_PUBLIC)
            .setOngoing(playing).setCategory(Notification.CATEGORY_TRANSPORT)
            .addAction(new Notification.Action.Builder(android.R.drawable.ic_media_previous, "Previous", command(2)).build())
            .addAction(new Notification.Action.Builder(playing ? android.R.drawable.ic_media_pause : android.R.drawable.ic_media_play,
                playing ? "Pause" : "Play", command(playing ? 0 : 1)).build())
            .addAction(new Notification.Action.Builder(android.R.drawable.ic_media_next, "Next", command(3)).build());
        if (open != null) builder.setContentIntent(PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_IMMUTABLE));
        if (session != null) builder.setStyle(new Notification.MediaStyle().setMediaSession(session.getSessionToken()).setShowActionsInCompactView(0, 1, 2));
        return builder.build();
    }
    private void refresh() {
        if (playing) {
            try { if (!foreground) promote(); }
            catch (RuntimeException denied) { action(0); return; }
            if (!focused) {
                focused = audio.requestAudioFocus(focus) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED;
                if (!focused) { action(0); return; }
            }
            // Bounded lock, renewed by real playback position updates.
            wake.acquire(60_000);
        } else {
            if (focused) audio.abandonAudioFocusRequest(focus);
            focused = false;
            if (wake.isHeld()) wake.release();
            if (foreground) stopForeground(STOP_FOREGROUND_DETACH);
            foreground = false;
        }
        session.setMetadata(new MediaMetadata.Builder().putString(MediaMetadata.METADATA_KEY_TITLE, title)
            .putString(MediaMetadata.METADATA_KEY_ARTIST, artist).putLong(MediaMetadata.METADATA_KEY_DURATION, duration).build());
        session.setPlaybackState(new PlaybackState.Builder()
            .setActions(PlaybackState.ACTION_PLAY | PlaybackState.ACTION_PAUSE | PlaybackState.ACTION_SKIP_TO_PREVIOUS | PlaybackState.ACTION_SKIP_TO_NEXT | PlaybackState.ACTION_STOP)
            .setState(playing ? PlaybackState.STATE_PLAYING : PlaybackState.STATE_PAUSED, position, playing ? 1 : 0).build());
        getSystemService(NotificationManager.class).notify(NOTIFICATION, notification());
    }
    @Override public void onDestroy() {
        instance = null;
        if (focused) audio.abandonAudioFocusRequest(focus);
        if (wake.isHeld()) wake.release();
        unregisterReceiver(noisy); session.release();
        getSystemService(NotificationManager.class).cancel(NOTIFICATION);
        action(0); super.onDestroy();
    }
    @Override public IBinder onBind(Intent intent) { return null; }

    /** Default network for this app, not merely any installed VPN application. */
    public static int vpn() {
        if (context == null) return 0;
        ConnectivityManager manager = context.getSystemService(ConnectivityManager.class);
        Network network = manager.getActiveNetwork();
        if (network == null) return -1;
        NetworkCapabilities caps = manager.getNetworkCapabilities(network);
        if (caps == null || !caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) return 0;
        LinkProperties links = manager.getLinkProperties(network);
        if (links == null) return 0;
        int result = 0;
        for (RouteInfo route : links.getRoutes()) {
            if (!route.isDefaultRoute()) continue;
            if (route.getDestination().getAddress() instanceof java.net.Inet4Address) result |= 1;
            else result |= 2;
        }
        return result;
    }
}
