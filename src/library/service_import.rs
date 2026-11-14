// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! A cancellable worker for reading an entire service account. Secrets go
//! through stdin, never command arguments, settings, or logs.

use std::{ffi::OsStr, path::PathBuf};

use gtk::{gio, glib, prelude::*};
use serde::{Deserialize, Serialize};

use crate::i18n::i18n;

#[derive(Serialize)]
pub struct Request {
    pub service: String,
    pub secret: String,
    pub account: String,
    pub download: bool,
    pub covers: bool,
    pub directory: PathBuf,
    pub browser: Option<String>,
    pub browser_profile: Option<PathBuf>,
    pub browse: bool,
}

pub use hopea_common::service_import::CatalogTrack;
#[cfg(test)]
pub use hopea_common::service_import::Collection;

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    CatalogTrack {
        track: CatalogTrack,
    },
    Ready,
    Warning {
        message: String,
    },
    Downloaded {
        key: String,
        path: Option<PathBuf>,
    },
    Metadata {
        key: String,
        cover: Option<String>,
        failed: bool,
    },
    Credentials {
        secret: String,
        account: String,
    },
    Progress {
        message: String,
    },
    Error {
        message: String,
    },
}

#[derive(Clone)]
pub struct Worker {
    pub process: gio::Subprocess,
    output: gio::DataInputStream,
    directory: PathBuf,
    _lifetime: std::rc::Rc<ProcessLifetime>,
}

impl Worker {
    pub async fn start(request: Request) -> Result<Self, String> {
        let python = std::env::var_os("HOPEA_IMPORT_PYTHON")
            .or_else(|| std::env::var_os("ANYSIK_IMPORT_PYTHON"))
            .or_else(|| {
                crate::beside_program("lib/hopea-import/bin/python3").map(|p| p.into_os_string())
            })
            .unwrap_or_else(|| "python3".into());
        let process = gio::Subprocess::newv(
            &[
                python.as_os_str(),
                OsStr::new("-I"),
                OsStr::new("-u"),
                OsStr::new("-c"),
                OsStr::new(include_str!("service_import.py")),
            ],
            gio::SubprocessFlags::STDIN_PIPE
                | gio::SubprocessFlags::STDOUT_PIPE
                | gio::SubprocessFlags::STDERR_SILENCE,
        )
        .map_err(|_| i18n("Unable to start the service importer. Install its dependencies as described in IMPORTING.md."))?;
        let worker = Self::from_process(process, request.directory.clone());
        let mut input = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        input.push(b'\n');
        if input.len() > 256 * 1024 {
            return Err(i18n("The sign-in data is too large"));
        }
        let stdin = worker.process.stdin_pipe().unwrap();
        let (_, _, error) = stdin
            .write_all_future(input, glib::Priority::DEFAULT)
            .await
            .map_err(|(_, err)| err.message().to_string())?;
        if let Some(error) = error {
            return Err(error.message().to_string());
        }
        if !request.browse {
            stdin
                .close_future(glib::Priority::DEFAULT)
                .await
                .map_err(|e| e.message().to_string())?;
        }
        Ok(worker)
    }

    fn from_process(process: gio::Subprocess, directory: PathBuf) -> Self {
        Self {
            output: gio::DataInputStream::new(&process.stdout_pipe().unwrap()),
            _lifetime: ProcessLifetime::new(&process),
            process,
            directory,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_process(process: gio::Subprocess, directory: PathBuf) -> Self {
        Self::from_process(process, directory)
    }

    pub async fn command(&self, command: serde_json::Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&command).map_err(|_| i18n("Invalid import command"))?;
        bytes.push(b'\n');
        let (_, _, error) = self
            .process
            .stdin_pipe()
            .unwrap()
            .write_all_future(bytes, glib::Priority::DEFAULT)
            .await
            .map_err(|_| i18n("The importer stopped unexpectedly"))?;
        if error.is_some() {
            return Err(i18n("The importer stopped unexpectedly"));
        }
        Ok(())
    }

    pub async fn next(&self) -> Result<Option<Event>, String> {
        let Some(line) = read_bounded_line(&self.output, 256 * 1024)
            .await
            .map_err(|_| i18n("The service importer sent too much data"))?
        else {
            return Ok(None);
        };
        serde_json::from_slice(&line)
            .map(Some)
            .map_err(|_| i18n("The service importer returned an invalid result"))
    }

    /// Only accept completed files from this job's download directory.
    pub fn accepts(&self, path: &std::path::Path) -> bool {
        let (Ok(root), Ok(file)) = (self.directory.canonicalize(), path.canonicalize()) else {
            return false;
        };
        file.parent() == Some(root.as_path())
            && file.extension() == Some(OsStr::new("flac"))
            && file.is_file()
    }
}

/// Never let GLib accumulate an unbounded line before checking its length.
/// Synchronous byte reads below only consume bytes already in the buffer.
pub(super) async fn read_bounded_line(
    stream: &gio::DataInputStream,
    limit: usize,
) -> Result<Option<Vec<u8>>, glib::Error> {
    let mut line = Vec::new();
    loop {
        if stream.available() == 0 && stream.fill_future(4096, glib::Priority::DEFAULT).await? == 0
        {
            return Ok((!line.is_empty()).then_some(line));
        }
        while stream.available() > 0 {
            let byte = gio::prelude::DataInputStreamExt::read_byte(stream, gio::Cancellable::NONE)?;
            if byte == b'\n' {
                return Ok(Some(line));
            }
            if line.len() == limit {
                return Err(glib::Error::new(
                    gio::IOErrorEnum::InvalidData,
                    "Importer message exceeds its size limit",
                ));
            }
            line.push(byte);
        }
    }
}

#[cfg(test)]
mod framing_tests {
    use super::*;
    #[test]
    fn importer_line_limit_applies_before_reading_an_entire_line() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let input = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_static(
                        b"abc\ndef\nlast",
                    ));
                    let stream = gio::DataInputStream::new(&input);
                    assert_eq!(
                        read_bounded_line(&stream, 3).await.unwrap(),
                        Some(b"abc".to_vec())
                    );
                    assert_eq!(
                        read_bounded_line(&stream, 3).await.unwrap(),
                        Some(b"def".to_vec())
                    );
                    assert_eq!(
                        read_bounded_line(&stream, 4).await.unwrap(),
                        Some(b"last".to_vec())
                    );
                    assert_eq!(read_bounded_line(&stream, 4).await.unwrap(), None);
                    let input = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(
                        vec![b'x'; 1024 * 1024],
                    ));
                    let stream = gio::DataInputStream::new(&input);
                    assert!(read_bounded_line(&stream, 256).await.is_err());
                    assert!(input.tell() <= 4096);
                })
            })
            .unwrap();
    }
}

pub async fn browser_session(service: &str, browser: &str) -> Result<(String, String), String> {
    let (reader, profile) = super::browser_import::saved_browser(browser)?;
    let worker = Worker::start(Request {
        service: service.into(),
        secret: String::new(),
        account: String::new(),
        download: false,
        covers: false,
        directory: glib::user_cache_dir(),
        browser: Some(reader),
        browser_profile: profile,
        browse: false,
    })
    .await?;
    let result = match worker.next().await? {
        Some(Event::Credentials { secret, account }) => Ok((secret, account)),
        Some(Event::Error { message }) => Err(message),
        _ => Err(i18n(
            "Unable to read the browser session. Sign in using the default browser profile and try again.",
        )),
    };
    worker
        .process
        .wait_future()
        .await
        .map_err(|_| i18n("The browser session reader stopped unexpectedly"))?;
    result
}

pub fn cancel(process: &gio::Subprocess) {
    #[cfg(unix)]
    process.send_signal(15);
    #[cfg(not(unix))]
    process.force_exit();
}

// Preview and background import share a process. Only the last owner may
// stop it; dropping a preview must not kill the active import.
pub(super) struct ProcessLifetime(gio::Subprocess);
impl ProcessLifetime {
    pub(super) fn new(process: &gio::Subprocess) -> std::rc::Rc<Self> {
        std::rc::Rc::new(Self(process.clone()))
    }
}
impl Drop for ProcessLifetime {
    fn drop(&mut self) {
        if self.0.identifier().is_some() {
            cancel(&self.0);
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Explicitly opt in on a signed-in workstation. No credentials or service
    /// URLs are printed or persisted; the resulting audio stays in /tmp.
    #[test]
    #[ignore = "requires explicit authorization to use the signed-in Zen profile"]
    fn live_zen_music_import() {
        let directory = PathBuf::from(
            std::env::var_os("HOPEA_LIVE_IMPORT_DIR").expect("temporary output directory"),
        );
        assert!(directory.starts_with("/tmp"));
        std::fs::create_dir_all(&directory).unwrap();
        let context = glib::MainContext::new();
        context.with_thread_default(|| context.block_on(async {
            assert_eq!(super::super::browser_import::default_browser().unwrap(), "zen");
            let (secret, account) = browser_session("vk", "zen").await.expect("Zen session verification");
            eprintln!("Zen session verified; reading catalogue");
            let worker = Worker::start(Request {
                service: "vk".into(), secret, account, download: true, covers: true,
                directory: directory.clone(), browser: None, browser_profile: None, browse: true,
            }).await.unwrap();
            let mut selected = None;
            let mut count = 0;
            let mut album_covers = Vec::new();
            loop {
                match worker.next().await.unwrap() {
                    Some(Event::CatalogTrack { track }) => {
                        count += 1;
                        for url in track.collections.iter().filter_map(|c| c.cover.as_ref()) {
                            if album_covers.len() < 2 && !album_covers.contains(url) { album_covers.push(url.clone()); }
                        }
                        if selected.is_none() && track.cover.is_some() { selected = Some(track); }
                    }
                    Some(Event::Ready) => break,
                    Some(Event::Progress { .. } | Event::Warning { .. }) => {},
                    Some(Event::Error { message }) => panic!("Catalogue failed: {}", message),
                    _ => panic!("Unexpected catalogue response"),
                }
            }
            eprintln!("Catalogue ready: {} songs; downloading one selected song", count);
            for url in &album_covers {
                super::super::import_history::fetch_cover(url).await
                    .unwrap_or_else(|reason| panic!("Album cover check: {}", reason));
            }
            eprintln!("Verified {} album covers", album_covers.len());
            drop(worker.clone()); // closing selection must leave downloading alive
            let track = selected.expect("at least one song with cover");
            let cover = super::super::import_history::fetch_cover(track.cover.as_deref().unwrap()).await.unwrap();
            worker.command(serde_json::json!({"key": track.key})).await.unwrap();
            let path = match worker.next().await.unwrap() {
                Some(Event::Downloaded { key, path: Some(path) }) if key == track.key => path,
                _ => panic!("Selected audio was not downloaded"),
            };
            assert!(worker.accepts(&path));
            assert!(std::fs::metadata(&path).unwrap().len() > 1024);
            super::super::import_history::save_cover(&path, &cover).await.unwrap();
            // A stable fixture name lets the isolated GTK test exercise the
            // actual service audio and artwork through Song::from_uri.
            std::fs::copy(&path, directory.join("selected.flac")).unwrap();
            std::fs::copy(path.with_extension("cover.jpg"), directory.join("selected.cover.jpg")).unwrap();
            worker.command(serde_json::json!({"finish": true})).await.unwrap();
            worker.process.wait_future().await.unwrap();
            eprintln!("Real Zen import completed: audio and cover saved in temporary test directory");
        })).unwrap();
    }
}

#[cfg(all(test, unix))]
mod lifetime_tests {
    use super::*;

    #[test]
    fn closing_preview_keeps_import_alive_and_last_owner_stops_worker() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let process = gio::Subprocess::newv(
                &[OsStr::new("sh"), OsStr::new("-c"), OsStr::new(
                    "while IFS= read -r line; do printf '%s\\n' '{\"event\":\"ready\"}'; done"
                )],
                gio::SubprocessFlags::STDIN_PIPE | gio::SubprocessFlags::STDOUT_PIPE
                    | gio::SubprocessFlags::STDERR_SILENCE,
            ).unwrap();
                    let worker = Worker::from_process(process.clone(), std::env::temp_dir());
                    let preview = worker.clone();
                    preview
                        .command(serde_json::json!({"preview":true}))
                        .await
                        .unwrap();
                    assert!(matches!(preview.next().await.unwrap(), Some(Event::Ready)));
                    drop(preview);
                    // This was a broken pipe as soon as the chooser released its clone.
                    worker
                        .command(serde_json::json!({"download":true}))
                        .await
                        .unwrap();
                    assert!(matches!(worker.next().await.unwrap(), Some(Event::Ready)));
                    drop(worker);
                    let result = futures::future::select(
                        Box::pin(process.wait_future()),
                        Box::pin(glib::timeout_future_seconds(5)),
                    )
                    .await;
                    if !matches!(result, futures::future::Either::Left((Ok(()), _))) {
                        process.force_exit();
                        panic!("last owner must stop its subprocess");
                    }
                    assert!(!process.is_successful());
                })
            })
            .unwrap();
    }
}
