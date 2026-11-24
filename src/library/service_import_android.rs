// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::os::{fd::AsRawFd, unix::net::UnixStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

extern "C" {
    fn hopea_android_import_start(fd: i32, mode: i32) -> i32;
    fn hopea_android_import_running(id: i32) -> i32;
    fn hopea_android_import_cancel(id: i32);
}

pub struct Process {
    id: i32,
    pub connection: gio::SocketConnection,
    cancelled: Arc<AtomicBool>,
}
impl Process {
    pub fn start(mode: i32) -> Result<super::Process, String> {
        let (local, remote) =
            UnixStream::pair().map_err(|_| i18n("Unable to start the service importer"))?;
        let socket = gio::Socket::from_fd(local.into()).map_err(|e| e.to_string())?;
        let id = unsafe { hopea_android_import_start(remote.as_raw_fd(), mode) };
        if id < 0 {
            return Err(i18n(
                "Another sign-in or import is still running. Close it and try again.",
            ));
        }
        Ok(super::Process::Android(std::rc::Rc::new(Self {
            id,
            connection: socket.connection_factory_create_connection(),
            cancelled: Arc::new(AtomicBool::new(false)),
        })))
    }
    pub fn running(&self) -> bool {
        unsafe { hopea_android_import_running(self.id) != 0 }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        unsafe {
            hopea_android_import_cancel(self.id);
        }
        let _ = self.connection.close(gio::Cancellable::NONE);
    }
    pub async fn wait(&self) -> Result<(), glib::Error> {
        for _ in 0..100 {
            if !self.running() {
                return Ok(());
            }
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
        }
        self.cancel();
        Err(glib::Error::new(
            gio::IOErrorEnum::TimedOut,
            "Importer did not stop",
        ))
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub async fn prepare_download(
    event: Event,
    directory: &std::path::Path,
    process: &super::Process,
) -> Result<Event, String> {
    let Event::Downloaded {
        key,
        path: Some(path),
    } = event
    else {
        return Ok(event);
    };
    if path.extension() == Some(OsStr::new("flac")) {
        return Ok(Event::Downloaded {
            key,
            path: Some(path),
        });
    }
    let super::Process::Android(process) = process else {
        return Err(i18n("Invalid import worker"));
    };
    let root = directory.to_owned();
    let cancelled = process.cancelled.clone();
    let path = gio::spawn_blocking(move || super::audio::convert(&root, &path, &cancelled))
        .await
        .map_err(|_| i18n("Unable to convert imported audio"))?
        .ok();
    // A missing decoder or invalid audio must not abort the remaining selection.
    // As with an unavailable download, keep the catalog entry for network matching.
    Ok(Event::Downloaded { key, path })
}
