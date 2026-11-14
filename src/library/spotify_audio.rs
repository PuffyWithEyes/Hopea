// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Direct Spotify audio in a disposable Rust subprocess. No account cache,
//! Connect listener, password, browser cookies or third-party audio provider.
use super::service_import::{self, CatalogTrack, Event};
use crate::i18n::i18n;
use gtk::{gio, glib, prelude::*};
use serde::Serialize;
use std::path::{Path, PathBuf};
const MAX_MESSAGE: u64 = 256 * 1024;
#[derive(Serialize)]
struct Start {
    secret: String,
    directory: PathBuf,
}

#[derive(Clone)]
pub struct SpotifyAudio {
    pub process: gio::Subprocess,
    output: gio::DataInputStream,
    _lifetime: std::rc::Rc<service_import::ProcessLifetime>,
}

impl SpotifyAudio {
    pub async fn start(secret: &str, directory: &Path) -> Result<Self, String> {
        let exe =
            std::env::current_exe().map_err(|_| i18n("Unable to find the Spotify importer"))?;
        let helper = exe
            .parent()
            .ok_or_else(|| i18n("Unable to find the Spotify importer"))?
            .join(format!("hopea-spotify{}", std::env::consts::EXE_SUFFIX));
        let process = gio::Subprocess::newv(
            &[helper.as_os_str()],
            gio::SubprocessFlags::STDIN_PIPE
                | gio::SubprocessFlags::STDOUT_PIPE
                | gio::SubprocessFlags::STDERR_SILENCE,
        )
        .map_err(|_| i18n("Unable to start the Spotify importer"))?;
        let worker = Self {
            output: gio::DataInputStream::new(&process.stdout_pipe().unwrap()),
            _lifetime: service_import::ProcessLifetime::new(&process),
            process,
        };
        worker
            .send(&Start {
                secret: secret.into(),
                directory: directory.into(),
            })
            .await?;
        Ok(worker)
    }

    async fn send(&self, value: &impl Serialize) -> Result<(), String> {
        let mut bytes =
            serde_json::to_vec(value).map_err(|_| i18n("Invalid Spotify import request"))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_MESSAGE as usize {
            return Err(i18n("Spotify import request is too large"));
        }
        let (_, _, error) = self
            .process
            .stdin_pipe()
            .unwrap()
            .write_all_future(bytes, glib::Priority::DEFAULT)
            .await
            .map_err(|_| i18n("The Spotify importer stopped"))?;
        if error.is_some() {
            return Err(i18n("The Spotify importer stopped"));
        }
        Ok(())
    }

    pub async fn download(&self, track: &CatalogTrack) -> Result<Option<PathBuf>, String> {
        self.send(track).await?;
        let line = service_import::read_bounded_line(&self.output, MAX_MESSAGE as usize)
            .await
            .map_err(|_| i18n("Unable to read the Spotify import result"))?
            .ok_or_else(|| i18n("The Spotify importer stopped"))?;
        if line.len() > MAX_MESSAGE as usize {
            return Err(i18n("Invalid Spotify import result"));
        }
        match serde_json::from_slice(&line) {
            Ok(Event::Downloaded { key, path }) if key == track.key => Ok(path),
            Ok(Event::Error { message }) => Err(message),
            _ => Err(i18n("Invalid Spotify import result")),
        }
    }

    pub async fn finish(&self) -> Result<(), String> {
        self.process
            .stdin_pipe()
            .unwrap()
            .close_future(glib::Priority::DEFAULT)
            .await
            .map_err(|_| i18n("The Spotify importer stopped"))?;
        self.process
            .wait_check_future()
            .await
            .map_err(|_| i18n("The Spotify importer stopped"))
    }
}
