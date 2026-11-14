// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
#define _GNU_SOURCE
#include <dlfcn.h>
#include <glib.h>
#include <gtk/gtk.h>
#include <gdk/android/gdkandroid.h>
extern void hopea_android_action(int action);

static jclass service;
static jmethodID update_method, vpn_method, game_method, usage_method;
static JavaVM *game_vm;
static void media_action(JNIEnv *env, jclass klass, jint action) {
  hopea_android_action(action);
}
static JNIEnv *bridge(void) {
  GdkDisplay *display = gdk_display_get_default();
  if (!display) return NULL;
  JNIEnv *env = gdk_android_display_get_env(display);
  if (service) return env;
  GListModel *windows = gtk_window_get_toplevels();
  jobject activity = NULL;
  for (guint i = 0; i < g_list_model_get_n_items(windows); i++) {
    GtkWindow *window = g_list_model_get_item(windows, i);
    GdkSurface *surface = gtk_native_get_surface(GTK_NATIVE(window));
    if (surface && GDK_IS_ANDROID_TOPLEVEL(surface))
      activity = gdk_android_toplevel_get_activity(GDK_ANDROID_TOPLEVEL(surface));
    g_object_unref(window);
    if (activity) break;
  }
  if (!activity) return NULL;
  if ((*env)->PushLocalFrame(env, 16) != 0) return NULL;
  jclass context = (*env)->GetObjectClass(env, activity);
  jmethodID get_loader = (*env)->GetMethodID(env, context, "getClassLoader", "()Ljava/lang/ClassLoader;");
  jobject loader = (*env)->CallObjectMethod(env, activity, get_loader);
  jclass loader_class = (*env)->GetObjectClass(env, loader);
  jmethodID load = (*env)->GetMethodID(env, loader_class, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;");
  jstring name = (*env)->NewStringUTF(env, "io.hopea.hopea.PlaybackService");
  jclass klass = (*env)->CallObjectMethod(env, loader, load, name);
  if ((*env)->ExceptionCheck(env) || !klass) goto failed;
  JNINativeMethod action = {"nativeAction", "(I)V", (void *)media_action};
  if ((*env)->RegisterNatives(env, klass, &action, 1) != JNI_OK) goto failed;
  jmethodID init = (*env)->GetStaticMethodID(env, klass, "initialize", "(Landroid/content/Context;)V");
  update_method = (*env)->GetStaticMethodID(env, klass, "update", "(ZLjava/lang/String;Ljava/lang/String;JJ)V");
  vpn_method = (*env)->GetStaticMethodID(env, klass, "vpn", "()I");
  game_method = (*env)->GetStaticMethodID(env, klass, "gameStatus", "()I");
  usage_method = (*env)->GetStaticMethodID(env, klass, "openUsageAccess", "()Z");
  if ((*env)->ExceptionCheck(env) || !init || !update_method || !vpn_method || !game_method || !usage_method) goto failed;
  (*env)->CallStaticVoidMethod(env, klass, init, activity);
  if ((*env)->ExceptionCheck(env)) goto failed;
  service = (*env)->NewGlobalRef(env, klass);
  if (!service || (*env)->GetJavaVM(env, &game_vm) != JNI_OK) goto failed;
  (*env)->PopLocalFrame(env, NULL);
  return env;
failed:
  (*env)->ExceptionClear(env);
  if (service) { (*env)->DeleteGlobalRef(env, service); service = NULL; }
  game_vm = NULL;
  (*env)->PopLocalFrame(env, NULL);
  g_warning("Android media integration could not initialize");
  return NULL;
}
static jstring java_string(JNIEnv *env, const char *text) {
  glong length = 0;
  gunichar2 *chars = g_utf8_to_utf16(text, -1, NULL, &length, NULL);
  jstring result = (*env)->NewString(env, (jchar *)chars, length);
  g_free(chars);
  return result;
}
void hopea_android_update(int playing, const char *title, const char *artist, int64_t duration, int64_t position) {
  JNIEnv *env = bridge();
  if (!env) { if (playing) hopea_android_action(0); return; }
  jstring t = java_string(env, title), a = java_string(env, artist);
  (*env)->CallStaticVoidMethod(env, service, update_method, (jboolean)playing, t, a, (jlong)duration, (jlong)position);
  (*env)->DeleteLocalRef(env, t);
  (*env)->DeleteLocalRef(env, a);
  if ((*env)->ExceptionCheck(env)) { (*env)->ExceptionClear(env); hopea_android_action(0); }
}
int hopea_android_vpn(void) {
  JNIEnv *env = bridge();
  if (!env) return 0;
  int result = (*env)->CallStaticIntMethod(env, service, vpn_method);
  if ((*env)->ExceptionCheck(env)) { (*env)->ExceptionClear(env); return 0; }
  return result;
}
// Called on GTK's thread before starting the worker. The worker must never
// access GTK/GDK objects or find an activity through gtk_window_get_toplevels.
void hopea_android_prepare_gaming(void) { (void)bridge(); }
int hopea_android_game_status(void) {
  if (!game_vm || !service || !game_method) return -2;
  JNIEnv *env = NULL;
  gboolean attached = FALSE;
  jint status = (*game_vm)->GetEnv(game_vm, (void **)&env, JNI_VERSION_1_6);
  if (status == JNI_EDETACHED) {
    if ((*game_vm)->AttachCurrentThread(game_vm, &env, NULL) != JNI_OK) return -2;
    attached = TRUE;
  } else if (status != JNI_OK) return -2;
  int result = (*env)->CallStaticIntMethod(env, service, game_method);
  if ((*env)->ExceptionCheck(env)) { (*env)->ExceptionClear(env); result = -2; }
  if (attached) (*game_vm)->DetachCurrentThread(game_vm);
  return result;
}
int hopea_android_open_usage_settings(void) {
  JNIEnv *env = bridge();
  if (!env) return 0;
  jboolean result = (*env)->CallStaticBooleanMethod(env, service, usage_method);
  if ((*env)->ExceptionCheck(env)) { (*env)->ExceptionClear(env); return 0; }
  return result == JNI_TRUE;
}
extern int hopea_android_main(void);
int main(int argc, char **argv) {
  Dl_info location;
  if (dladdr((void *)&hopea_android_main, &location) && location.dli_fname) {
    char *directory = g_path_get_dirname(location.dli_fname);
    g_setenv("GST_PLUGIN_PATH_1_0", directory, TRUE);
    g_free(directory);
  }
  // Android cannot execute the desktop plugin scanner subprocess.
  g_setenv("GST_REGISTRY_FORK", "no", TRUE);
  return hopea_android_main();
}
