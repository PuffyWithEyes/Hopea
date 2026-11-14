// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use super::import_picker::DuplicateChoice;
use super::*;
use library::{
    import_history::{self, History, Imported},
    service_import::{CatalogTrack, Event, Request, Worker},
};

thread_local! {
    // Survives closing/reopening the main window while the app is held.
    static JOB: RefCell<Option<Rc<ImportJob>>> = const { RefCell::new(None) };
}

struct ImportJob {
    review: super::import_review::Review,
    dialog: adw::AlertDialog,
    label: gtk::Label,
    bar: gtk::ProgressBar,
    stage: gtk::Label,
    counts: gtk::Label,
    failed_label: gtk::Label,
    eta: gtk::Label,
    timing: RefCell<Option<(std::time::Instant, usize, usize)>>,
    paused_for_game: Cell<bool>,
    cancelled: Cell<bool>,
    finished: Cell<bool>,
    processes: RefCell<Vec<gio::Subprocess>>,
}

impl ImportJob {
    fn new() -> Rc<Self> {
        let label = gtk::Label::builder()
            .wrap(true)
            .max_width_chars(60)
            .xalign(0.0)
            .build();
        let bar = gtk::ProgressBar::new();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        label.add_css_class("heading");
        let stage = gtk::Label::builder().wrap(true).xalign(0.0).build();
        stage.add_css_class("dim-label");
        let counts = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .use_markup(true)
            .build();
        let failed_label = gtk::Label::builder().xalign(0.0).wrap(true).build();
        failed_label.add_css_class("error");
        failed_label.add_css_class("heading");
        failed_label.set_visible(false);
        let eta = gtk::Label::builder().xalign(0.0).wrap(true).build();
        eta.add_css_class("caption");
        content.append(&stage);
        content.append(&label);
        content.append(&bar);
        content.append(&counts);
        content.append(&failed_label);
        content.append(&eta);
        let review = super::import_review::Review::new();
        content.append(&review.widget);
        let dialog = adw::AlertDialog::builder()
            .heading(i18n("Music Import"))
            .extra_child(&content)
            .close_response("cancel")
            .build();
        dialog.add_response("cancel", &i18n("_Cancel"));
        let job = Rc::new(Self {
            review,
            dialog,
            label,
            bar,
            stage,
            counts,
            failed_label,
            eta,
            timing: RefCell::new(None),
            paused_for_game: Cell::new(false),
            cancelled: Cell::new(false),
            finished: Cell::new(false),
            processes: RefCell::new(Vec::new()),
        });
        let weak = Rc::downgrade(&job);
        job.dialog.connect_response(None, move |_, response| {
            if response != "stop" && response != "cancel" {
                return;
            }
            if let Some(job) = weak.upgrade() {
                job.cancelled.set(true);
                for process in job.processes.borrow().iter() {
                    if process.identifier().is_some() {
                        library::service_import::cancel(process);
                    }
                }
            }
        });
        let weak = Rc::downgrade(&job);
        glib::timeout_add_seconds_local(1, move || {
            let Some(job) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if job.finished.get() {
                return glib::ControlFlow::Break;
            }
            job.update_eta();
            glib::ControlFlow::Continue
        });
        job
    }
    fn start_timing(&self, total: usize) {
        self.timing
            .replace(Some((std::time::Instant::now(), 0, total)));
        self.update_eta();
    }
    fn update_eta(&self) {
        if self.paused_for_game.get() {
            self.eta
                .set_text(&i18n("Import will resume after the game ends"));
            return;
        }
        let timing = self.timing.borrow();
        let Some((start, done, total)) = *timing else {
            return;
        };
        let elapsed = start.elapsed().as_secs_f64();
        self.eta
            .set_text(&match remaining_time(elapsed, done, total) {
                Some(seconds) if seconds < 60 => i18n("About a minute remaining"),
                Some(seconds) if seconds < 3600 => i18n_f(
                    "About {} min remaining",
                    &[&((seconds + 59) / 60).to_string()],
                ),
                Some(seconds) => i18n_f(
                    "About {} h {} min remaining",
                    &[
                        &(seconds / 3600).to_string(),
                        &((seconds % 3600 + 59) / 60).to_string(),
                    ],
                ),
                None => i18n("Estimating remaining time…"),
            });
    }
    fn completed(&self, done: usize) {
        if let Some((_, count, total)) = self.timing.borrow_mut().as_mut() {
            *count = done;
            self.bar.set_fraction(done as f64 / (*total).max(1) as f64);
        }
        self.update_eta();
    }
    fn counters(&self, added: usize, skipped: usize, updated: usize, wanted: usize, failed: usize) {
        let normal = i18n_f(
            "Shared {} · Skipped {} · Covers {} · Names only {}",
            &[
                &added.to_string(),
                &skipped.to_string(),
                &updated.to_string(),
                &wanted.to_string(),
            ],
        );
        let failure = i18n_f("Failed {}", &[&failed.to_string()]);
        self.counts.set_text(&normal);
        self.failed_label.set_text(&failure);
        self.failed_label.set_visible(failed > 0);
    }
    fn enable_stop(&self) {
        self.show_close();
        self.dialog.add_response("stop", &i18n("_Stop Import"));
        self.dialog
            .set_response_appearance("stop", adw::ResponseAppearance::Destructive);
    }
    fn show_close(&self) {
        self.dialog.set_close_response("close");
        if self.dialog.has_response("cancel") {
            self.dialog.remove_response("cancel");
        }
        if !self.dialog.has_response("close") {
            self.dialog.add_response("close", &i18n("_Close"));
        }
    }
    fn hide_preparation(&self) {
        // Programmatic handoff to selection must not act like the user's Cancel.
        self.show_close();
        self.dialog.close();
    }
    fn message(&self, message: &str) {
        self.label.set_text(message);
        self.bar.pulse();
    }
    fn check(&self) -> Result<(), String> {
        if self.cancelled.get() {
            Err(i18n("Import cancelled. Completed songs were kept."))
        } else {
            Ok(())
        }
    }
}

fn current_window(panel: &LibraryPanel) -> Option<Window> {
    panel.window().or_else(|| {
        gio::Application::default()?
            .downcast::<gtk::Application>()
            .ok()?
            .active_window()?
            .downcast()
            .ok()
    })
}

fn update_import_buttons() {
    if let Some(app) =
        gio::Application::default().and_then(|app| app.downcast::<gtk::Application>().ok())
    {
        for window in app
            .windows()
            .into_iter()
            .filter_map(|w| w.downcast::<Window>().ok())
        {
            window.set_import_active(LibraryPanel::import_is_running());
        }
    }
}

impl LibraryPanel {
    pub fn import_is_running() -> bool {
        JOB.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|job| !job.finished.get() && !job.cancelled.get())
        })
    }

    pub fn show_import_job(&self) {
        JOB.with(|slot| {
            if let Some(job) = slot.borrow().as_ref() {
                AdwDialogExt::present(&job.dialog, current_window(self).as_ref());
            } else if let Some(window) = current_window(self) {
                window.add_toast(i18n("No import has been started"));
            }
        });
    }

    pub(super) async fn begin_account_import(&self, request: Request) {
        if JOB.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|job| !job.finished.get())
        }) {
            self.show_import_job();
            return;
        }
        let job = ImportJob::new();
        JOB.with(|slot| slot.replace(Some(job.clone())));
        update_import_buttons();
        self.imp().busy.set(true);
        job.message(&i18n("Reading your library for selection…"));
        AdwDialogExt::present(&job.dialog, current_window(self).as_ref());
        let panel = self.clone();
        let app = gio::Application::default();
        let hold = app.as_ref().map(|app| app.hold());
        glib::spawn_future_local(async move {
            let _hold = hold;
            let result = panel.run_account_job(&request, &job).await;
            let result = if job.cancelled.get() {
                job.check().map(|_| String::new())
            } else {
                result
            };
            job.finished.set(true);
            job.show_close();
            update_import_buttons();
            for process in job.processes.borrow_mut().drain(..) {
                if process.identifier().is_some() {
                    library::service_import::cancel(&process);
                }
            }
            job.bar.set_visible(false);
            job.eta.set_visible(false);
            job.stage.set_visible(false);
            if job.dialog.has_response("stop") {
                job.dialog.remove_response("stop");
            }
            match result {
                Ok(summary) => {
                    job.dialog.set_heading(Some(&i18n("Import Complete")));
                    job.label.set_text(&summary);
                    if !job.failed_label.is_visible() {
                        job.label.add_css_class("success");
                    }
                }
                Err(error) => {
                    job.label.set_text(&error);
                    job.label.add_css_class(if job.cancelled.get() {
                        "warning"
                    } else {
                        "error"
                    });
                    job.dialog.set_heading(Some(&i18n(if job.cancelled.get() {
                        "Import Stopped"
                    } else {
                        "Import Not Completed"
                    })));
                }
            }
            panel.imp().busy.set(false);
            panel.imp().upload_revealer.set_reveal_child(false);
            panel.refresh_shared_music().await;
            if let Some(window) = current_window(&panel) {
                window.show_import_result_toast();
                window.look_for_wanted();
            }
        });
    }

    async fn run_account_job(
        &self,
        request: &Request,
        job: &Rc<ImportJob>,
    ) -> Result<String, String> {
        let mut history = History::load()?;
        let worker = Worker::start(Request {
            service: request.service.clone(),
            secret: request.secret.clone(),
            account: request.account.clone(),
            download: request.download,
            covers: request.covers,
            directory: request.directory.clone(),
            browser: None,
            browser_profile: None,
            browse: true,
        })
        .await?;
        job.processes.borrow_mut().push(worker.process.clone());
        job.check()?;
        let spotify = if request.service == "spotify" && request.download {
            let audio =
                library::spotify_audio::SpotifyAudio::start(&request.secret, &request.directory)
                    .await?;
            job.processes.borrow_mut().push(audio.process.clone());
            Some(audio)
        } else {
            None
        };
        let mut tracks = Vec::new();
        let mut catalog_warning: Option<String> = None;
        loop {
            job.check()?;
            match worker.next().await? {
                Some(Event::CatalogTrack { track }) => {
                    tracks.push(track);
                }
                Some(Event::Progress { message }) => job.message(&message),
                Some(Event::Warning { message }) => {
                    let warnings = catalog_warning.get_or_insert_with(String::new);
                    if warnings.len().saturating_add(message.len()) > 16 * 1024 * 1024 {
                        return Err(i18n("The service returned too many import warnings"));
                    }
                    if !warnings.is_empty() {
                        warnings.push('\n');
                    }
                    warnings.push_str(&message);
                }
                Some(Event::Ready) => break,
                Some(Event::Error { message }) => return Err(message),
                _ => return Err(i18n("The service could not finish reading the library")),
            }
        }
        job.check()?;
        let preview = super::import_preview::Preview::new(Some(worker.clone()), spotify.clone());
        let artwork = preview.artwork.clone();
        let Some(selected) = choose_tracks(
            current_window(self).as_ref(),
            tracks,
            &history,
            &request.service,
            request.covers,
            catalog_warning.as_deref(),
            Some(preview),
            Some(job),
        )
        .await
        else {
            job.cancelled.set(true);
            return Err(i18n("Import cancelled. No selected songs were imported."));
        };
        job.check()?;
        job.enable_stop();
        job.message(&i18n("Starting background import…"));
        if let Some(window) = current_window(self) {
            window.add_toast(i18n("Import started. Use Import Progress to check it."));
        }
        // A selection is the explicit start; all subsequent progress is non-modal.
        if request.download {
            if !library::allowed(&self.settings()) {
                return Err(i18n("Audio import needs the sharing network. Connect to the VPN required by your settings, or import song names only."));
            }
            if library::client().is_none() {
                self.join().await;
            }
            if self.imp().me.borrow().is_none() {
                if let Some(client) = library::client() {
                    self.finish_joining(client).await;
                }
            }
            if library::client().is_none() || self.imp().me.borrow().is_none() {
                return Err(i18n(
                    "Unable to connect to the sharing network. No audio files were imported.",
                ));
            }
        }
        // Check ownership before downloading anything. A stale history entry
        // must not cause an audio download in the cover-update phase.
        job.message(&i18n("Saving selected albums…"));
        let mut albums = library::import_albums::ImportedAlbums::begin(
            &request.service,
            &selected.tracks,
            &selected.collections,
        )
        .await?;
        job.check()?;
        job.message(&i18n("Checking existing songs before updating covers…"));
        let mut prepared = Vec::with_capacity(selected.len());
        let decisions = selected.decisions;
        for track in selected.tracks {
            job.check()?;
            let key = import_history::key(&request.service, &track.key);
            let audio =
                import_history::audio_path(&request.directory, &request.service, &track.key);
            let mut record = history.0.get(&key).cloned().unwrap_or_else(|| Imported {
                node_id: library::shared_files::id_for(&gio::File::for_path(&audio).uri()),
                title: track.title.clone(),
                artist: track.artist.clone(),
                ..Default::default()
            });
            let mut existing = None;
            if let (Some(id), Some(client)) = (record.node_id, library::client()) {
                match client.track(id).await {
                    Ok(t)
                        if t.mine
                            && record.matches_audio(&t.sha256, &t.title, t.artist.as_deref()) =>
                    {
                        existing = Some(t);
                    }
                    Ok(_) | Err(library::ClientError::NotFound) => record.node_id = None,
                    Err(_) => {
                        return Err(i18n(
                            "Unable to check previously imported audio; retry when connected.",
                        ))
                    }
                }
            }
            if decisions.contains_key(&track.key) {
                existing = None;
            }
            prepared.push((track, record, existing));
        }
        prepared.sort_by_key(|(_, record, existing)| {
            existing.is_none() && (request.download || !record.metadata)
        });
        let (mut added, mut skipped, mut updated, mut wanted, mut failed, mut cover_errors) =
            (0, 0, 0, 0, 0, 0);
        let mut missing_covers = 0;
        let mut replacements = Vec::new();
        let protected_existing: std::collections::HashSet<i64> = decisions
            .values()
            .filter(|d| d.choice != DuplicateChoice::Incoming)
            .filter_map(|d| d.candidate.id)
            .collect();
        let total = prepared.len();
        let mut failures = Vec::new();
        if let Some(warning) = catalog_warning {
            // Full details belong to the warnings view, not the result label.
            failures.push(warning.lines().next().unwrap_or_default().to_owned());
        }
        job.start_timing(total);
        for (index, (track, mut record, existing)) in prepared.into_iter().enumerate() {
            job.check()?;
            // Finish an in-flight file safely, then defer the next background
            // transfer/FFmpeg job while gaming. Cancel remains responsive.
            let paused = std::time::Instant::now();
            while crate::gaming::active() {
                job.paused_for_game.set(true);
                job.stage.set_text(&i18n("Paused while gaming"));
                job.eta
                    .set_text(&i18n("Import will resume after the game ends"));
                glib::timeout_future(std::time::Duration::from_secs(1)).await;
                job.check()?;
            }
            job.paused_for_game.set(false);
            if let Some((started, _, _)) = job.timing.borrow_mut().as_mut() {
                *started += paused.elapsed();
            }
            job.stage.set_text(&if !request.covers {
                i18n("Importing without cover art")
            } else if existing.is_some() || (!request.download && record.metadata) {
                i18n("Updating existing cover art")
            } else {
                i18n("Downloading music and cover art")
            });
            job.label.set_text(&format!(
                "{}\n{}",
                track.title,
                track.artist.as_deref().unwrap_or("")
            ));
            job.bar.set_show_text(true);
            job.bar.set_text(Some(&format!("{} / {total}", index + 1)));
            job.counters(added, skipped, updated, wanted, failed);
            job.completed(index);
            let key = import_history::key(&request.service, &track.key);
            let audio =
                import_history::audio_path(&request.directory, &request.service, &track.key);
            record.title = track.title.clone();
            record.artist = track.artist.clone();
            record.album = track.album.clone();
            if let Some(decision) = decisions
                .get(&track.key)
                .filter(|d| d.choice == DuplicateChoice::Existing)
            {
                let client =
                    library::client().ok_or_else(|| i18n("Connect to use the existing version"))?;
                let id = decision
                    .candidate
                    .id
                    .ok_or_else(|| i18n("The existing audio is unavailable"))?;
                let old = client.track(id).await.map_err(|_| {
                    i18n("The existing track is no longer available; select another version")
                })?;
                if !decision.candidate.matches(&old) || !old.local {
                    return Err(i18n(
                        "The existing audio has changed or is unavailable; select another version",
                    ));
                }
                if !albums.contains(&track.key) && !old.favorite {
                    client
                        .set_favorite(id, true)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                record.node_id = Some(id);
                record.node_sha256 = Some(old.sha256);
                record.metadata = true;
                albums
                    .update(
                        &track.key,
                        &record,
                        &audio,
                        request.covers.then_some(&artwork.covers),
                    )
                    .await?;
                history.0.insert(key, record);
                history.save().await?;
                skipped += 1;
                continue;
            }
            // VK's section list can omit art which reload_audio supplies.
            // Ask for metadata separately, including for already imported audio.
            let mut cover_url = track.cover.clone().filter(|_| request.covers);
            let mut metadata_failed = false;
            if request.covers && request.service == "vk" && cover_url.is_none() {
                worker
                    .command(serde_json::json!({"key": track.key, "metadata": true}))
                    .await?;
                match worker.next().await? {
                    Some(Event::Metadata { key, cover, failed }) if key == track.key => {
                        cover_url = cover.or(cover_url);
                        metadata_failed = failed;
                    }
                    Some(Event::Error { message }) => return Err(message),
                    _ => return Err(i18n("The importer stopped while reading cover art")),
                }
            }
            job.check()?;
            let cover = if let Some(url) = cover_url.as_deref() {
                match artwork.covers.get(url).await {
                    Ok(bytes) => Some(bytes),
                    Err(reason) => {
                        if failures.len() < 5 {
                            failures.push(i18n_f("Cover art: {}", &[&reason]));
                        }
                        None
                    }
                }
            } else {
                None
            };
            if (cover_url.is_some() || metadata_failed) && cover.is_none() {
                cover_errors += 1;
            } else if request.covers && cover_url.is_none() {
                missing_covers += 1;
            }
            job.check()?;
            let hash = cover.as_ref().map(import_history::cover_hash);
            if let Some(existing) = existing {
                record.metadata = true;
                record.node_sha256 = Some(existing.sha256.clone());
                if let Some(bytes) = cover
                    .as_ref()
                    .filter(|_| hash != record.cover_hash || !existing.has_cover)
                {
                    if job
                        .review
                        .cover_choice(&existing, bytes, &audio, current_window(self).as_ref())
                        .await
                        .is_err()
                    {
                        cover_errors += 1;
                    }
                }
                skipped += 1;
                albums
                    .update(
                        &track.key,
                        &record,
                        &audio,
                        request.covers.then_some(&artwork.covers),
                    )
                    .await?;
                history.0.insert(key, record);
                history.save().await?;
                continue;
            }
            if !request.download && record.metadata {
                if let Some(bytes) = cover.as_ref().filter(|_| hash != record.cover_hash) {
                    import_history::save_cover(&audio, bytes).await?;
                    record.cover_hash = hash;
                    updated += 1;
                } else {
                    skipped += 1;
                }
                albums
                    .update(
                        &track.key,
                        &record,
                        &audio,
                        request.covers.then_some(&artwork.covers),
                    )
                    .await?;
                history.0.insert(key, record);
                history.save().await?;
                continue;
            }
            let mut path = None;
            if let Some(spotify) = spotify.as_ref() {
                let result = spotify.download(&track).await;
                job.check()?;
                path = result?;
            } else if request.download {
                worker
                    .command(serde_json::json!({"key": track.key}))
                    .await?;
                match worker.next().await? {
                    Some(Event::Downloaded {
                        key: returned,
                        path: downloaded,
                    }) if returned == track.key => path = downloaded,
                    Some(Event::Error { message }) => return Err(message),
                    _ => {
                        job.check()?;
                        return Err(i18n("The importer stopped while downloading a song"));
                    }
                }
            }
            job.check()?;
            if let Some(path) = path {
                if !worker.accepts(&path) || path != audio {
                    return Err(i18n("The importer returned an unexpected file"));
                }
                let file = gio::File::for_path(&path);
                if let Some(bytes) = &cover {
                    import_history::save_cover(&audio, bytes).await?;
                }
                let mut upload_cover = UploadCover::default();
                if let (Some(bytes), Some(client)) = (&cover, library::client()) {
                    match client.add_cover(bytes.clone()).await {
                        Ok(name) => upload_cover.cover = Some(name),
                        Err(_) => cover_errors += 1,
                    }
                }
                let result = self.upload(&file, None, &upload_cover).await;
                let duplicate = matches!(&result, Upload::Duplicate(_));
                match result {
                    Upload::Shared(t) | Upload::Duplicate(t) => {
                        // Persist standalone-song favorites even while the main window
                        // is closed. Album membership must not imply a star.
                        if !albums.contains(&track.key) && !duplicate {
                            if let Some(client) = library::client() {
                                client
                                    .set_favorite(t.id, true)
                                    .await
                                    .map_err(|error| error.to_string())?;
                            }
                        }
                        library::shared_files::remember(&file.uri(), t.id);
                        if let Some(decision) = decisions
                            .get(&track.key)
                            .filter(|d| d.choice == DuplicateChoice::Incoming)
                        {
                            if decision
                                .candidate
                                .id
                                .is_some_and(|id| id != t.id && !protected_existing.contains(&id))
                            {
                                replacements.push((decision.candidate.clone(), t.clone()));
                            }
                        }
                        record.node_id = Some(t.id);
                        record.node_sha256 = Some(t.sha256.clone());
                        record.metadata = true;
                        let mut applied = upload_cover.cover.is_some();
                        if duplicate {
                            job.review.duplicate(&t);
                            applied = false;
                            if let Some(bytes) = &cover {
                                if job
                                    .review
                                    .cover_choice(&t, bytes, &audio, current_window(self).as_ref())
                                    .await
                                    .is_err()
                                {
                                    cover_errors += 1;
                                }
                            }
                        }
                        if applied {
                            record.cover_hash = hash;
                        }
                        albums
                            .update(
                                &track.key,
                                &record,
                                &audio,
                                request.covers.then_some(&artwork.covers),
                            )
                            .await?;
                        history.0.insert(key, record);
                        history.save().await?;
                        if let Some(window) = current_window(self) {
                            if !duplicate {
                                if albums.contains(&track.key) {
                                    window.mark_shared_file(&file.uri(), t.id);
                                } else {
                                    window.add_shared_file(&file, t.id);
                                }
                            }
                            if let Some(bytes) = cover.as_ref().filter(|_| applied) {
                                window.refresh_import_cover(t.id, bytes);
                            }
                        }
                        if duplicate {
                            skipped += 1;
                        } else {
                            added += 1;
                        }
                        // Updating the model was missing from the original service importer.
                        self.refresh_shared_music().await;
                    }
                    Upload::Failed(reason) => {
                        failed += 1;
                        if failures.len() < 5 {
                            failures.push(reason);
                        }
                    }
                }
            } else {
                if request.download {
                    failed += 1;
                    if failures.len() < 5 {
                        failures.push(i18n_f("Audio unavailable: {}", &[&track.title]));
                    }
                }
                if !record.metadata {
                    library::import::add(vec![library::import::Wanted {
                        title: track.title.clone(),
                        artist: track.artist.clone(),
                        tried_at: 0,
                    }]);
                    wanted += 1;
                } else {
                    skipped += 1;
                }
                record.metadata = true;
                if let Some(bytes) = cover.as_ref() {
                    import_history::save_cover(&audio, bytes).await?;
                    record.cover_hash = hash;
                }
                albums
                    .update(
                        &track.key,
                        &record,
                        &audio,
                        request.covers.then_some(&artwork.covers),
                    )
                    .await?;
                history.0.insert(key, record);
                history.save().await?;
            }
        }
        let mut replaced = std::collections::HashSet::new();
        for (old, new) in replacements {
            job.check()?;
            let Some(id) = old.id else {
                continue;
            };
            if replaced.contains(&id) {
                continue;
            }
            let result = library::import_history::replace_preserving_file(&old, &new).await;
            match result {
                Ok(()) => {
                    replaced.insert(id);
                    library::shared_files::forget(id);
                    if let Some(window) = current_window(self) {
                        window.remove_replaced_library_track(id);
                    }
                }
                Err(reason) => {
                    failed += 1;
                    if failures.len() < 5 {
                        failures.push(i18n_f("Both versions were kept: {}", &[&reason]));
                    }
                }
            }
        }
        job.completed(total);
        job.counters(added, skipped, updated, wanted, failed);
        if let Some(spotify) = spotify.as_ref() {
            spotify.finish().await?;
        }
        worker.command(serde_json::json!({"finish": true})).await?;
        worker
            .process
            .wait_future()
            .await
            .map_err(|_| i18n("The importer stopped unexpectedly"))?;
        job.processes.borrow_mut().clear();
        let summary = i18n_f("Shared {} audio files; skipped {}; covers updated {}; names only {}; failed {}; cover errors {}; no cover supplied by service {}. Names without audio cannot be shared yet.",
            &[&added.to_string(), &skipped.to_string(), &updated.to_string(), &wanted.to_string(), &failed.to_string(), &cover_errors.to_string(), &missing_covers.to_string()]);
        let summary = format!(
            "{}\n{}",
            summary,
            i18n_f("{} albums saved in My Albums", &[&albums.len().to_string()])
        );
        Ok(if failures.is_empty() {
            summary
        } else {
            format!("{summary}\n\n{}", failures.join("\n"))
        })
    }

    pub(crate) async fn refresh_shared_music(&self) {
        let Some(client) = library::client() else {
            return;
        };
        // The old panel may belong to a closed window. Refresh the current one.
        let panel = current_window(self)
            .map(|w| w.import_panel())
            .unwrap_or_else(|| self.clone());
        let imp = panel.imp();
        let query = imp.search_entry.text().trim().to_string();
        if query.is_empty() {
            panel.search();
            return;
        }
        let generation = imp.generation.get();
        if let Ok(page) = client.discover(&query, 0, false, panel.order()).await {
            if imp.generation.get() == generation && imp.search_entry.text().trim() == query {
                panel.show_page(&query, 0, &page.tracks, page.total);
            }
        }
    }
}

#[cfg(test)]
impl LibraryPanel {
    pub async fn test_picker_and_background_controls(&self) {
        fn descendants(widget: &gtk::Widget) -> Vec<gtk::Widget> {
            let mut result = vec![widget.clone()];
            let mut child = widget.first_child();
            while let Some(widget) = child {
                result.extend(descendants(&widget));
                child = widget.next_sibling();
            }
            result
        }
        let cover_url = "https://i.scdn.co/hopea-ui-fixture.jpg";
        super::import_preview::Artwork::seed_test_cover(cover_url);
        let tracks: Vec<CatalogTrack> = (0..5000)
            .map(|i| CatalogTrack {
                key: i.to_string(),
                title: format!("Track {i}"),
                duration_ms: None,
                artist: Some("Artist".into()),
                album: (i < 3).then(|| "Album A".into()),
                album_artist: None,
                track_number: None,
                cover: Some(cover_url.into()),
                collections: vec![library::service_import::Collection {
                    position: Some(i % 3),
                    cover: Some(cover_url.into()),
                    key: if i < 3 {
                        "album:a".into()
                    } else if i < 6 {
                        "playlist:b".into()
                    } else {
                        "library".into()
                    },
                    kind: if i < 3 {
                        "album".into()
                    } else if i < 6 {
                        "playlist".into()
                    } else {
                        "library".into()
                    },
                    title: if i < 3 {
                        "Album A".into()
                    } else if i < 6 {
                        "Playlist B".into()
                    } else {
                        "Liked songs".into()
                    },
                }],
            })
            .collect();
        let window = current_window(self).unwrap();
        window.set_default_size(960, 800);
        GtkWindowExt::present(&window);
        let parent = window.clone();
        let selection = glib::spawn_future_local(async move {
            choose_tracks(
                Some(&parent),
                tracks,
                &History::default(),
                "vk",
                true,
                Some("Only part of the library was read.\n• Witness: VK refused access to this collection for the signed-in account.\n• Combat Stimulus: VK refused access to this collection for the signed-in account.\nReadTimeout"),
                None,
                None,
            )
            .await
        });
        for _ in 0..100 {
            if window
                .visible_dialog()
                .is_some_and(|d| d.widget_name() == "import-picker")
            {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        let dialog = window.visible_dialog().unwrap();
        assert_eq!(dialog.widget_name(), "import-picker");
        window.set_default_size(960, 800);
        for _ in 0..100 {
            if dialog
                .child()
                .is_some_and(|child| child.is_mapped() && child.width() > 0)
                && window.width() == 960
                && window.height() == 800
            {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            dialog.child().unwrap().is_mapped(),
            "the picker content must actually be visible"
        );
        let named = |name: &str| {
            descendants(dialog.upcast_ref())
                .into_iter()
                .find(|w| w.widget_name() == name)
                .unwrap()
        };
        let list = named("import-track-list")
            .downcast::<gtk::ListView>()
            .unwrap();
        let all = named("import-select-all")
            .downcast::<gtk::CheckButton>()
            .unwrap();
        let confirm = named("import-confirm").downcast::<gtk::Button>().unwrap();
        let count = named("import-selected-count")
            .downcast::<gtk::Label>()
            .unwrap();
        let search = named("import-search")
            .downcast::<gtk::SearchEntry>()
            .unwrap();
        assert_eq!(list.model().unwrap().n_items(), 4994);
        for _ in 0..40 {
            if descendants(list.upcast_ref())
                .iter()
                .filter_map(|w| w.downcast_ref::<gtk::Picture>())
                .any(|p| p.paintable().is_some())
            {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
        }
        assert!(
            descendants(list.upcast_ref())
                .iter()
                .filter_map(|w| w.downcast_ref::<gtk::Picture>())
                .any(|p| p.paintable().is_some()),
            "song rows must display artwork"
        );
        let album_shelf = named("import-albums");
        assert!(
            descendants(&album_shelf)
                .iter()
                .filter_map(|w| w.downcast_ref::<gtk::Picture>())
                .any(|p| p.paintable().is_some()),
            "album cards must display artwork"
        );
        crate::dialogs::test_fit(&window, &dialog).await;
        window.set_default_size(960, 800);
        for _ in 0..50 {
            glib::timeout_future(std::time::Duration::from_millis(40)).await;
            if window.width() >= 950 && window.height() >= 790 {
                break;
            }
        }
        for card in descendants(&album_shelf)
            .iter()
            .filter(|w| w.has_css_class("import-collection"))
        {
            let viewport = album_shelf
                .clone()
                .downcast::<gtk::ScrolledWindow>()
                .unwrap()
                .child()
                .unwrap();
            let rect = card.compute_bounds(&viewport).unwrap();
            assert!(
                rect.y() >= 0.0 && rect.y() + rect.height() <= viewport.height() as f32,
                "album cards must fit vertically: {:?} in {}",
                rect,
                album_shelf.height()
            );
        }
        assert!(
            list.height() >= 240,
            "reserve room for choosing songs: list {}, shelf {}, window {}x{}, dialog {}x{}",
            list.height(),
            album_shelf.height(),
            window.width(),
            window.height(),
            dialog.width(),
            dialog.height()
        );
        // A smaller desktop window must not sacrifice the bottom of cards or
        // their selection controls to the fixed minimum of the song list.
        window.set_default_size(900, 640);
        glib::timeout_future(std::time::Duration::from_millis(300)).await;
        for widget in descendants(&album_shelf).iter().filter(|w| {
            w.has_css_class("import-collection")
                || w.widget_name() == "import-collection-selection"
                || w.widget_name() == "import-cover-picture"
        }) {
            let rect = widget.compute_bounds(&album_shelf).unwrap();
            assert!(
                rect.y() >= 0.0 && rect.y() + rect.height() <= album_shelf.height() as f32,
                "album content must fit in a short window: {:?} / {}",
                rect,
                album_shelf.height()
            );
        }
        assert!(list.height() >= 128);
        window.set_default_size(960, 800);
        glib::timeout_future(std::time::Duration::from_millis(300)).await;

        assert!(all.is_active());
        assert_eq!(count.text(), i18n_f("{} of {} selected", &["4994", "5000"]));
        assert!(confirm.is_sensitive());
        assert_eq!(
            named("import-catalog-warning")
                .downcast::<gtk::Label>()
                .unwrap()
                .text(),
            "Some music needs attention"
        );
        let warnings_button = named("import-show-warnings")
            .downcast::<gtk::Button>()
            .unwrap();
        assert_eq!(warnings_button.label().as_deref(), Some("Warnings (3)"));
        warnings_button.emit_clicked();
        glib::timeout_future(std::time::Duration::from_millis(300)).await;
        let warnings_dialog = window.visible_dialog().unwrap();
        assert_eq!(warnings_dialog.widget_name(), "import-warnings");
        crate::dialogs::test_fit(&window, &warnings_dialog).await;
        let warning_rows: Vec<_> = descendants(warnings_dialog.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<adw::ExpanderRow>().ok())
            .collect();
        assert_eq!(warning_rows.len(), 3);
        assert_eq!(warning_rows[0].title(), "Combat Stimulus");
        assert!(warning_rows[0].has_css_class("import-unavailable"));
        warning_rows[0].set_expanded(true);
        if let Some(path) = std::env::var_os("HOPEA_TEST_WARNINGS_SCREENSHOT") {
            warnings_dialog.queue_draw();
            glib::timeout_future(std::time::Duration::from_secs(2)).await;
            let canvas = warnings_dialog.child().unwrap();
            assert!(canvas.is_mapped() && canvas.width() > 0 && canvas.height() > 0);
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gdk::RGBA::parse("#242424").unwrap(),
                &gtk::graphene::Rect::new(0.0, 0.0, canvas.width() as f32, canvas.height() as f32),
            );
            let mut child = canvas.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                canvas.snapshot_child(&widget, &snapshot);
            }
            window
                .renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
        warnings_dialog.close();
        glib::timeout_future(std::time::Duration::from_millis(300)).await;
        assert!(!descendants(dialog.upcast_ref())
            .iter()
            .filter_map(|w| w.downcast_ref::<gtk::Button>())
            .any(|b| {
                b.label().as_deref() == Some(i18n("Next").as_str())
                    || b.label().as_deref() == Some(i18n("Previous").as_str())
            }));
        list.scroll_to(4993, gtk::ListScrollFlags::NONE, None);
        for _ in 0..100 {
            if descendants(list.upcast_ref())
                .iter()
                .filter_map(|w| w.downcast_ref::<adw::ActionRow>())
                .any(|r| r.title() == "Track 4999")
            {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        let rows = descendants(list.upcast_ref());
        assert!(
            rows.iter()
                .filter_map(|w| w.downcast_ref::<adw::ActionRow>())
                .any(|r| r.title() == "Track 4999"),
            "scroll reaches the last track without a Next button"
        );
        assert!(
            rows.iter()
                .filter(|w| w.widget_name() == "import-track-selection")
                .count()
                < 500,
            "offscreen rows must be recycled"
        );
        drop(rows);
        search.set_text("Track 4999");
        search.emit_by_name::<()>("search-changed", &[]);
        assert_eq!(list.model().unwrap().n_items(), 1);
        all.set_active(false);
        assert_eq!(count.text(), i18n_f("{} of {} selected", &["4993", "5000"]));
        search.set_text("");
        search.emit_by_name::<()>("search-changed", &[]);
        assert!(
            all.is_inconsistent(),
            "changing the filter must preserve selection"
        );
        all.set_active(true);
        assert_eq!(count.text(), i18n_f("{} of {} selected", &["4994", "5000"]));
        for check in descendants(dialog.upcast_ref())
            .into_iter()
            .filter(|w| w.widget_name() == "import-collection-selection")
            .filter_map(|w| w.downcast::<gtk::CheckButton>().ok())
        {
            assert!(
                !check.is_active(),
                "albums start unchecked and Select All does not select them"
            );
            check.set_active(true);
        }
        all.set_active(false);
        assert!(
            confirm.is_sensitive(),
            "collection selections must be preserved"
        );
        assert_eq!(count.text(), i18n_f("{} of {} selected", &["6", "5000"]));
        for check in descendants(dialog.upcast_ref())
            .into_iter()
            .filter(|w| w.widget_name() == "import-collection-selection")
            .filter_map(|w| w.downcast::<gtk::CheckButton>().ok())
        {
            check.set_active(false);
        }
        assert!(!confirm.is_sensitive());
        let album_check = descendants(dialog.upcast_ref())
            .into_iter()
            .filter(|w| w.widget_name() == "import-collection-selection")
            .filter_map(|w| w.downcast::<gtk::CheckButton>().ok())
            .find(|c| c.tooltip_text().is_some_and(|t| t.contains("Album A")))
            .unwrap();
        album_check.set_active(true);
        assert!(confirm.is_sensitive());
        assert_eq!(count.text(), i18n_f("{} of {} selected", &["3", "5000"]));
        descendants(dialog.upcast_ref())
            .into_iter()
            .filter(|w| w.widget_name() == "import-open-collection")
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.tooltip_text().as_deref() == Some("Album A"))
            .unwrap()
            .emit_clicked();
        assert_eq!(list.model().unwrap().n_items(), 3);
        assert!(
            !album_shelf.is_mapped(),
            "opening an album gives its tracks the full list area"
        );

        for _ in 0..100 {
            if descendants(list.upcast_ref())
                .iter()
                .filter_map(|w| w.downcast_ref::<adw::ActionRow>())
                .any(|r| r.title() == "Track 0")
            {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            descendants(list.upcast_ref())
                .iter()
                .filter_map(|w| w.downcast_ref::<adw::ActionRow>())
                .all(|row| row.activatable_widget().is_none()),
            "album songs cannot be individually unchecked"
        );
        for _ in 0..40 {
            if descendants(list.upcast_ref())
                .iter()
                .filter_map(|w| w.downcast_ref::<gtk::Picture>())
                .filter(|p| p.paintable().is_some())
                .count()
                == 3
            {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        let rows: Vec<_> = descendants(list.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<adw::ActionRow>().ok())
            .collect();
        assert_eq!(rows.len(), 3);
        for row in rows {
            let widgets = descendants(row.upcast_ref());
            assert!(
                widgets.iter().any(|w| w
                    .downcast_ref::<gtk::Picture>()
                    .is_some_and(|p| p.paintable().is_some())),
                "every album song needs its artwork"
            );
            assert!(
                widgets
                    .iter()
                    .any(|w| w.downcast_ref::<gtk::Button>().is_some_and(|b| b
                        .icon_name()
                        .as_deref()
                        == Some("hopea-media-playback-start-symbolic")
                        && b.is_sensitive())),
                "every album song needs a Play button"
            );
        }
        all.set_active(false);
        assert!(!album_check.is_active());
        assert!(!album_check.is_inconsistent());
        assert_eq!(count.text(), i18n_f("{} of {} selected", &["0", "5000"]));
        all.set_active(true);
        assert!(album_check.is_active());
        assert!(!all.is_inconsistent());
        named("import-all-songs")
            .downcast::<gtk::Button>()
            .unwrap()
            .emit_clicked();
        assert_eq!(list.model().unwrap().n_items(), 4994);
        assert!(!all.is_active());
        assert_eq!(count.text(), i18n_f("{} of {} selected", &["3", "5000"]));
        // The screenshot is opt-in and contains synthetic data only.
        if let Ok(path) = std::env::var("HOPEA_TEST_PICKER_SCREENSHOT") {
            named("import-all-songs")
                .downcast::<gtk::Button>()
                .unwrap()
                .emit_clicked();
            glib::timeout_future(std::time::Duration::from_secs(2)).await;
            let canvas = dialog.child().unwrap();
            assert!(list.is_mapped() && list.height() > 100);
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gdk::RGBA::parse("#242424").unwrap(),
                &gtk::graphene::Rect::new(0.0, 0.0, canvas.width() as f32, canvas.height() as f32),
            );
            let mut child = canvas.first_child();
            while let Some(widget) = child {
                canvas.snapshot_child(&widget, &snapshot);
                child = widget.next_sibling();
            }
            window
                .renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
        confirm.emit_clicked();
        let selected = selection.await.unwrap().unwrap();
        assert_eq!(
            selected.iter().map(|t| t.key.as_str()).collect::<Vec<_>>(),
            vec!["0", "1", "2"]
        );
        super::import_picker::Picker::test_duplicate_scopes(&window).await;
        let preparing = ImportJob::new();
        assert!(preparing.dialog.has_response("cancel"));
        assert!(!preparing.dialog.has_response("close"));
        assert_eq!(preparing.dialog.close_response(), "cancel");
        #[cfg(unix)]
        let preparation_process = gio::Subprocess::newv(
            &[std::ffi::OsStr::new("sleep"), std::ffi::OsStr::new("30")],
            gio::SubprocessFlags::NONE,
        )
        .unwrap();
        #[cfg(unix)]
        preparing
            .processes
            .borrow_mut()
            .push(preparation_process.clone());
        AdwDialogExt::present(&preparing.dialog, Some(&window));
        preparing
            .dialog
            .emit_by_name::<()>("response", &[&"cancel"]);
        assert!(preparing.cancelled.get(), "Cancel must stop preparation");
        assert!(preparing.check().is_err());
        #[cfg(unix)]
        {
            use futures::future::{select, Either};
            let stopped = select(
                Box::pin(preparation_process.wait_future()),
                Box::pin(glib::timeout_future(std::time::Duration::from_secs(3))),
            )
            .await;
            if !matches!(stopped, Either::Left((Ok(()), _))) {
                preparation_process.force_exit();
                panic!("Cancel must terminate the catalogue worker");
            }
        }
        preparing.dialog.close();
        let job = ImportJob::new();
        job.start_timing(10);
        job.timing.replace(Some((
            std::time::Instant::now() - std::time::Duration::from_secs(20),
            2,
            10,
        )));
        job.update_eta();
        assert!(job.eta.text().contains("remaining"));
        job.paused_for_game.set(true);
        job.update_eta();
        assert_eq!(job.eta.text(), "Import will resume after the game ends");
        job.paused_for_game.set(false);
        job.update_eta();
        assert!(job.eta.text().contains("remaining"));
        job.counters(2, 1, 0, 0, 1);
        assert!(job.failed_label.is_visible() && job.failed_label.has_css_class("error"));
        assert!(!job.counts.has_css_class("error"));

        JOB.with(|slot| slot.replace(Some(job.clone())));
        update_import_buttons();
        let buttons: Vec<gtk::Button> = descendants(window.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .filter(|button| button.action_name().as_deref() == Some("win.import-progress"))
            .filter(|button| {
                button.icon_name().as_deref() == Some("hopea-folder-download-symbolic")
            })
            .collect();
        assert_eq!(buttons.len(), 2);
        assert!(buttons.iter().all(|button| button.get_visible()));
        // A reopened window must inherit the current job's state.
        let reopened = Window::new(&window.application().unwrap());
        assert!(Self::import_is_running());
        assert!(descendants(reopened.upcast_ref())
            .iter()
            .filter_map(|w| w.downcast_ref::<gtk::Button>())
            .filter(|button| button.action_name().as_deref() == Some("win.import-progress"))
            .all(|button| button.get_visible()));
        reopened.close();
        #[cfg(unix)]
        for _ in 0..2 {
            let process = gio::Subprocess::newv(
                &[std::ffi::OsStr::new("sleep"), std::ffi::OsStr::new("30")],
                gio::SubprocessFlags::NONE,
            )
            .unwrap();
            job.processes.borrow_mut().push(process);
        }
        AdwDialogExt::present(&job.dialog, Some(&window));
        job.hide_preparation();
        assert!(
            !job.cancelled.get(),
            "opening selection must not cancel the import"
        );
        job.enable_stop();
        AdwDialogExt::present(&job.dialog, Some(&window));
        assert!(job.dialog.has_response("close"));
        assert!(!job.dialog.has_response("cancel"));
        assert!(!job.dialog.has_response("background"));
        job.dialog.emit_by_name::<()>("response", &[&"close"]);
        assert!(
            !job.cancelled.get(),
            "hiding progress must not cancel import"
        );
        assert!(Self::import_is_running());
        let children = job.processes.borrow().clone();
        for process in &children {
            assert!(
                process.identifier().is_some(),
                "closing progress must keep workers alive"
            );
        }
        assert_eq!(
            job.dialog.response_appearance("stop"),
            adw::ResponseAppearance::Destructive
        );
        job.dialog.emit_by_name::<()>("response", &[&"stop"]);
        assert!(job.cancelled.get());
        assert!(!Self::import_is_running());
        job.finished.set(true);
        update_import_buttons();
        assert!(buttons.iter().all(|button| !button.get_visible()));
        for process in children {
            use futures::future::{select, Either};
            let result = select(
                Box::pin(process.wait_future()),
                Box::pin(glib::timeout_future(std::time::Duration::from_secs(3))),
            )
            .await;
            if !matches!(result, Either::Left((Ok(()), _))) {
                process.force_exit();
                panic!("Cleanup must stop both catalogue and audio workers");
            }
        }
        job.dialog.close();
        JOB.with(|slot| slot.borrow_mut().take());
        update_import_buttons();
    }
}

async fn choose_tracks(
    parent: Option<&Window>,
    tracks: Vec<CatalogTrack>,
    history: &History,
    service_name: &str,
    covers: bool,
    catalog_warning: Option<&str>,
    preview: Option<Rc<super::import_preview::Preview>>,
    job: Option<&ImportJob>,
) -> Option<super::import_picker::Selection> {
    let preview = preview.unwrap_or_else(|| super::import_preview::Preview::new(None, None));
    let mut known: Vec<_> = history
        .0
        .values()
        .map(|item| library::duplicates::Candidate {
            id: item.node_id,
            sha256: item.node_sha256.clone(),
            title: item.title.clone(),
            artist: item.artist.clone(),
            duration_ms: None,
        })
        .collect();
    if let Some(client) = library::client() {
        let mut offset = 0;
        while let Ok(page) = client.local_tracks(offset).await {
            if job.is_some_and(|job| job.cancelled.get()) {
                return None;
            }
            if page.tracks.is_empty() {
                break;
            }
            offset += page.tracks.len() as u32;
            known.extend(
                page.tracks
                    .into_iter()
                    .map(|track| library::duplicates::Candidate {
                        id: Some(track.id),
                        sha256: Some(track.sha256.clone()),
                        title: track.title,
                        artist: track.artist,
                        duration_ms: Some(track.duration_ms),
                    }),
            );
            if u64::from(offset) >= page.total {
                break;
            }
        }
    }
    let known = library::duplicates::Index::new(known);
    let duplicates = tracks
        .iter()
        .map(|track| known.find(track))
        .collect::<Vec<_>>();
    let imported = tracks
        .iter()
        .map(|track| {
            history
                .0
                .contains_key(&import_history::key(service_name, &track.key))
        })
        .collect();
    if let Some(job) = job {
        if job.cancelled.get() {
            return None;
        }
        job.hide_preparation();
    }
    super::import_picker::Picker::choose(
        parent,
        tracks,
        duplicates,
        imported,
        covers,
        catalog_warning,
        preview,
    )
    .await
}

fn remaining_time(elapsed: f64, completed: usize, total: usize) -> Option<u64> {
    if completed < 2 || elapsed < 3.0 || completed >= total {
        return None;
    }
    Some((elapsed / completed as f64 * (total - completed) as f64).ceil() as u64)
}

#[cfg(test)]
mod eta_tests {
    #[test]
    fn estimate_needs_samples_and_tracks_stalls() {
        assert_eq!(super::remaining_time(30.0, 1, 10), None);
        assert_eq!(super::remaining_time(20.0, 2, 10), Some(80));
        assert_eq!(super::remaining_time(40.0, 2, 10), Some(160));
        assert_eq!(super::remaining_time(40.0, 10, 10), None);
    }
}
