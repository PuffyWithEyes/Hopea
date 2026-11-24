// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
package io.hopea.hopea;

import android.app.Activity;
import android.os.*;
import android.graphics.Bitmap;
import android.net.Uri;
import android.net.http.SslError;
import android.view.WindowManager;
import android.webkit.*;
import android.widget.*;
import org.json.*;
import java.nio.charset.StandardCharsets;
import java.util.Locale;

/** VK has no supported music-session handoff from other installed clients.
 * This window signs in to VK itself and returns only that window's cookies.
 * It never reads another app/browser's storage or intercepts its intents. */
public final class VkSignInActivity extends Activity {
    private WebView web;
    private ParcelFileDescriptor pipe;
    private int job;
    private boolean completed;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final Runnable checkCancelled = new Runnable() {
        public void run() {
            if (ImportBridge.running(job) == 0) { finish(); return; }
            main.postDelayed(this, 500);
        }
    };
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_SECURE);
        job = getIntent().getIntExtra("job", -1);
        pipe = ImportBridge.takeLoginPipe(job);
        if (pipe == null) { finish(); return; }
        try { WebView.setDataDirectorySuffix("hopea-vk-sign-in"); }
        catch (IllegalStateException alreadyInitialized) { }
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        TextView address = new TextView(this);
        address.setText("Sign in to VK to let Hopea read your music.");
        address.setPadding(16, 16, 16, 16);
        layout.addView(address);
        Button cancel = new Button(this);
        cancel.setText("Cancel");
        cancel.setOnClickListener(v -> finish());
        layout.addView(cancel);
        web = new WebView(this);
        layout.addView(web, new LinearLayout.LayoutParams(-1, 0, 1));
        setContentView(layout);
        WebSettings settings = web.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(false);
        settings.setSaveFormData(false);
        settings.setCacheMode(WebSettings.LOAD_NO_CACHE);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        CookieManager cookies = CookieManager.getInstance();
        cookies.setAcceptCookie(true);
        cookies.setAcceptThirdPartyCookies(web, false);
        web.setWebViewClient(new WebViewClient() {
            @Override public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                return !allowed(request.getUrl());
            }
            @Override public void onReceivedSslError(WebView view, SslErrorHandler handler, SslError error) {
                handler.cancel();
            }
            @Override public void onPageStarted(WebView view, String url, Bitmap icon) {
                Uri uri = Uri.parse(url);
                if (!allowed(uri)) { view.stopLoading(); return; }
                address.setText("VK sign-in · " + uri.getHost());
            }
            @Override public void onPageFinished(WebView view, String url) {
                if (!allowed(Uri.parse(url)) || completed) return;
                returnSession(cookies);
            }
        });
        // Ephemeral sign-in, not a token/session cache. Clear an interrupted run.
        cookies.removeAllCookies(done -> { if (!isFinishing()) web.loadUrl("https://vk.com/audio"); });
        main.post(checkCancelled);
        main.postDelayed(this::finish, 360000);
    }
    static boolean allowed(Uri uri) {
        if (!"https".equals(uri.getScheme()) || uri.getUserInfo() != null
            || (uri.getPort() != -1 && uri.getPort() != 443)) return false;
        String host = uri.getHost();
        if (host == null) return false;
        host = host.toLowerCase(Locale.ROOT);
        return host.equals("vk.com") || host.endsWith(".vk.com")
            || host.equals("vk.ru") || host.endsWith(".vk.ru")
            || host.equals("vkid.ru") || host.endsWith(".vkid.ru");
    }
    private void returnSession(CookieManager manager) {
        try {
            JSONArray values = new JSONArray();
            String account = "";
            boolean signedIn = false;
            for (String domain : new String[]{"vk.com", "vk.ru", "m.vk.com", "m.vk.ru"}) {
                String header = manager.getCookie("https://" + domain + "/");
                if (header == null || header.length() > 65536) continue;
                for (String pair : header.split(";")) {
                    int eq = pair.indexOf('=');
                    if (eq < 1) continue;
                    String name = pair.substring(0, eq).trim(), value = pair.substring(eq + 1).trim();
                    if ((name.equals("remixsid") || name.equals("remixsid6")) && !value.isEmpty()) signedIn = true;
                    if (name.equals("remixmid") && value.matches("[1-9][0-9]*")) account = value;
                    values.put(new JSONObject().put("name", name).put("value", value).put("domain", "." + domain).put("path", "/"));
                }
            }
            if (!signedIn) return;
            JSONObject secret = new JSONObject().put("cookies", values).put("viewer_id", account)
                .put("user_agent", web.getSettings().getUserAgentString());
            byte[] response = (new JSONObject().put("event", "credentials").put("secret", secret.toString())
                .put("account", account).toString() + "\n").getBytes(StandardCharsets.UTF_8);
            if (response.length > 256 * 1024) { finish(); return; }
            completed = true;
            ParcelFileDescriptor output = pipe; pipe = null;
            new Thread(() -> {
                try (ParcelFileDescriptor.AutoCloseOutputStream stream = new ParcelFileDescriptor.AutoCloseOutputStream(output)) {
                    stream.write(response);
                } catch (Exception ignored) { }
                main.post(this::finish);
            }, "Hopea VK authorization").start();
        } catch (JSONException invalid) { finish(); }
    }
    @Override public void onDestroy() {
        main.removeCallbacksAndMessages(null);
        if (web != null) {
            web.stopLoading(); web.clearCache(true); web.clearHistory(); web.destroy();
            CookieManager.getInstance().removeAllCookies(null);
            WebStorage.getInstance().deleteAllData();
        }
        if (pipe != null) { try { pipe.close(); } catch (Exception ignored) { } }
        ImportBridge.loginFinished(job);
        super.onDestroy();
    }
}
