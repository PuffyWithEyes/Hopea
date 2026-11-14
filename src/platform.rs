// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

/// Set relocatable bundle paths before GTK, GStreamer or worker threads start.
#[cfg(target_os = "windows")]
pub fn init() {
    use std::{env, path::PathBuf};
    let Some(bin) = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
    else {
        return;
    };
    let Some(root) = bin.parent() else { return };
    for (key, path) in [
        ("GSETTINGS_SCHEMA_DIR", root.join("share/glib-2.0/schemas")),
        ("GST_PLUGIN_PATH_1_0", root.join("lib/gstreamer-1.0")),
        (
            "GST_PLUGIN_SCANNER_1_0",
            root.join("libexec/gstreamer-1.0/gst-plugin-scanner.exe"),
        ),
        (
            "HOPEA_IMPORT_PYTHON",
            root.join("lib/hopea-import/python.exe"),
        ),
    ] {
        if env::var_os(key).is_none() && path.exists() {
            env::set_var(key, path);
        }
    }
    let mut dirs = vec![root.join("share")];
    if let Some(existing) = env::var_os("XDG_DATA_DIRS") {
        dirs.extend(env::split_paths(&existing));
    }
    if let Ok(dirs) = env::join_paths(dirs) {
        env::set_var("XDG_DATA_DIRS", dirs);
    }
    // FFmpeg and the private Python DLLs must be found even when Explorer
    // launched the player with an unrelated working directory.
    let mut dirs = vec![bin.clone()];
    if let Some(existing) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&existing));
    }
    if let Ok(dirs) = env::join_paths(dirs) {
        env::set_var("PATH", dirs);
    }
    // A copied loader cache contains absolute paths from the build machine.
    // Rebuild it from the bundle's trusted loader directory after relocation.
    let loaders = root.join("lib/gdk-pixbuf-2.0/2.10.0/loaders");
    if loaders.is_dir() {
        env::set_var("GDK_PIXBUF_MODULEDIR", &loaders);
        let mut dlls: Vec<_> = std::fs::read_dir(&loaders)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "dll"))
            .collect();
        dlls.sort();
        if !dlls.is_empty() {
            use std::os::windows::process::CommandExt;
            if let Ok(output) = std::process::Command::new(bin.join("gdk-pixbuf-query-loaders.exe"))
                .args(&dlls)
                .creation_flags(0x08000000)
                .output()
            {
                if output.status.success() && output.stdout.len() <= 1024 * 1024 {
                    let cache = gtk::glib::user_cache_dir().join("hopea");
                    let file = cache.join("pixbuf-loaders.cache");
                    if std::fs::create_dir_all(&cache).is_ok()
                        && std::fs::write(&file, output.stdout).is_ok()
                    {
                        env::set_var("GDK_PIXBUF_MODULE_FILE", file);
                    }
                }
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn init() {}
