// SPDX-FileCopyrightText: 2022  Emmanuele Bassi
// SPDX-License-Identifier: GPL-3.0-or-later

use std::cell::{Cell, RefCell};

use glib::clone;
use gst::prelude::*;
use gtk::{gio, glib, prelude::*, subclass::prelude::*};
use log::{debug, warn};

use crate::{
    audio::{Controller, PlaybackState, RepeatMode, Song},
    library,
};

mod imp {
    use glib::{ParamSpec, ParamSpecBoolean, Value};
    use once_cell::sync::Lazy;

    use super::*;

    #[derive(Debug, Default)]
    pub struct WaveformGenerator {
        pub song: RefCell<Option<Song>>,
        pub peaks: RefCell<Option<Vec<(f64, f64)>>>,
        pub pipeline: RefCell<Option<(gst::Element, gst::bus::BusWatchGuard)>>,
        pub generation: Cell<u64>,
        pub remote_pending: Cell<bool>,
        pub remote_missing: Cell<bool>,
        pub remote_retries: Cell<u8>,
        pub remote_retry: RefCell<Option<glib::SourceId>>,
        pub audio_started: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WaveformGenerator {
        const NAME: &'static str = "WaveformGenerator";
        type Type = super::WaveformGenerator;
    }

    impl ObjectImpl for WaveformGenerator {
        fn dispose(&self) {
            if let Some(retry) = self.remote_retry.take() {
                retry.remove();
            }
            if let Some((pipeline, _bus_watch)) = self.pipeline.take() {
                pipeline.send_event(gst::event::Eos::new());
                match pipeline.set_state(gst::State::Null) {
                    Ok(_) => {}
                    Err(err) => warn!("Unable to set existing pipeline to Null state: {}", err),
                }
            }
        }

        fn properties() -> &'static [ParamSpec] {
            static PROPERTIES: Lazy<Vec<ParamSpec>> =
                Lazy::new(|| vec![ParamSpecBoolean::builder("has-peaks").read_only().build()]);

            PROPERTIES.as_ref()
        }

        fn property(&self, _id: usize, pspec: &ParamSpec) -> Value {
            match pspec.name() {
                "has-peaks" => self.obj().peaks().is_some().to_value(),
                _ => unimplemented!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires temporary XDG directories and the generated GSettings schema"]
    fn remote_waveform_waits_for_verification_after_audio_starts() {
        use std::{
            io::{BufRead, Write},
            sync::{
                atomic::{AtomicBool, AtomicUsize, Ordering},
                Arc,
            },
            time::{Duration, Instant},
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (count, done) = (requests.clone(), stop.clone());
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done.load(Ordering::SeqCst) && Instant::now() < deadline {
                let Ok((mut socket, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = std::io::BufReader::new(socket.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
                        break;
                    }
                }
                if !request.contains("/tracks/42/peaks ") {
                    // Connecting the client also schedules publication of
                    // saved lyrics; this fixture has no lyrics endpoint.
                    write!(
                        socket,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .unwrap();
                    continue;
                }
                // The initial request precedes verification of the download.
                let (status, body) = if count.fetch_add(1, Ordering::SeqCst) < 2 {
                    ("404 Not Found", "{}")
                } else {
                    ("200 OK", "[[0.25,0.5]]")
                };
                write!(socket, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        glib::MainContext::default().block_on(async {
            library::set_client(Some(
                library::LibraryClient::new(&format!("http://{address}"), "test").unwrap(),
            ));
            let track = serde_json::from_value(serde_json::json!({
                "id":42, "title":"Waveform regression", "artist":null, "album":null,
                "duration_ms":1000, "format":"flac", "codec":"flac", "sample_rate":44100,
                "channels":2, "size":1024, "sha256":"waveform-regression",
                "has_cover":false, "uploader":"Test", "uploaded_at":0,
                "mine":false, "local":false, "favorite":false, "sources":1
            }))
            .unwrap();
            let song = Song::from_remote(&track, "http://127.0.0.1/unused-audio", None);
            let generator = WaveformGenerator::new();
            generator.imp().song.replace(Some(song));
            generator.fetch_remote_peaks(42);
            for _ in 0..100 {
                if !generator.imp().remote_pending.get() {
                    break;
                }
                glib::timeout_future(Duration::from_millis(10)).await;
            }
            assert!(generator.imp().remote_missing.get());
            assert!(generator.peaks().is_none());
            assert_eq!(requests.load(Ordering::SeqCst), 1);
            generator.set_position(1, false);
            for _ in 0..500 {
                if generator.peaks().is_some() {
                    break;
                }
                glib::timeout_future(Duration::from_millis(10)).await;
            }
            assert_eq!(generator.peaks(), Some(vec![(0.25, 0.5)]));
            for position in 2..100 {
                generator.set_position(position, false);
            }
            glib::timeout_future(Duration::from_millis(50)).await;
            assert_eq!(requests.load(Ordering::SeqCst), 3);
            // A late cache read from the previous selection cannot replace it.
            generator
                .load_peaks_cache("/nonexistent/old-waveform", u64::MAX)
                .await;
            assert_eq!(generator.peaks(), Some(vec![(0.25, 0.5)]));
            library::set_client(None);
        });
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
    }
}

glib::wrapper! {
    pub struct WaveformGenerator(ObjectSubclass<imp::WaveformGenerator>);
}

impl Default for WaveformGenerator {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Controller for WaveformGenerator {
    fn set_playback_state(&self, _playback_state: &PlaybackState) {}

    fn set_song(&self, song: &Song) {
        let imp = self.imp();
        imp.generation.set(imp.generation.get().wrapping_add(1));
        imp.remote_pending.set(false);
        imp.remote_missing.set(false);
        imp.remote_retries.set(0);
        if let Some(retry) = imp.remote_retry.take() {
            retry.remove();
        }
        imp.audio_started.set(false);
        imp.peaks.replace(None);
        if let Some((pipeline, _watch)) = imp.pipeline.take() {
            let _ = pipeline.set_state(gst::State::Null);
        }
        self.imp().song.replace(Some(song.clone()));
        self.notify("has-peaks");
        self.load_peaks();
    }

    fn set_position(&self, position: u64, _notify: bool) {
        if position > 0 {
            self.imp().audio_started.set(true);
            self.retry_remote_peaks();
        }
    }
    fn set_repeat_mode(&self, _mode: RepeatMode) {}
}

impl WaveformGenerator {
    pub fn new() -> Self {
        WaveformGenerator::default()
    }

    pub fn peaks(&self) -> Option<Vec<(f64, f64)>> {
        (*self.imp().peaks.borrow()).as_ref().cloned()
    }

    fn save_peaks(&self) {
        if let Some(peaks) = self.peaks() {
            let song = match self.imp().song.borrow().as_ref() {
                Some(s) => s.clone(),
                None => {
                    self.notify("has-peaks");
                    return;
                }
            };

            if let Some(uuid) = song.uuid() {
                let mut cache = crate::paths::cache_dir();
                cache.push("waveforms");
                glib::mkdir_with_parents(&cache, 0o755);

                cache.push(format!("{}.json", uuid));

                let j = serde_json::to_string(&peaks).unwrap();
                let file = gio::File::for_path(&cache);
                file.replace_contents_async(
                    j,
                    None,
                    false,
                    gio::FileCreateFlags::NONE,
                    gio::Cancellable::NONE,
                    move |_| {
                        debug!("Waveform cached at: {:?}", &cache);
                    },
                );
            }
        }

        self.notify("has-peaks");
    }

    async fn load_peaks_cache(&self, cache: impl AsRef<std::path::Path>, generation: u64) {
        let file = gio::File::for_path(&cache);
        let bytes = crate::utils::read_file_limited(&file, 4 * 1024 * 1024).await;
        if self.imp().generation.get() != generation {
            return;
        }
        if let Ok(bytes) = bytes {
            match serde_json::from_slice::<Vec<(f64, f64)>>(&bytes[..]) {
                Ok(p) if !p.is_empty() => {
                    debug!("Loaded cached waveform file");
                    self.imp().peaks.replace(Some(p));
                    self.notify("has-peaks");
                }
                _ => {
                    debug!("Cached waveform is empty or invalid");
                    self.generate_peaks();
                }
            }
        } else {
            debug!("Could not read cached waveform file");
            self.generate_peaks();
        }
    }

    fn load_peaks(&self) {
        let generation = self.imp().generation.get();
        let song = match self.imp().song.borrow().as_ref() {
            Some(s) => s.clone(),
            None => return,
        };

        if let Some(uuid) = song.uuid() {
            let mut cache = crate::paths::cache_dir();
            cache.push("waveforms");
            cache.push(format!("{}.json", uuid));

            let ctx = glib::MainContext::default();
            ctx.spawn_local(clone!(
                #[weak(rename_to = this)]
                self,
                async move { this.load_peaks_cache(&cache, generation).await }
            ));
        } else {
            self.generate_peaks();
        }
    }

    fn retry_remote_peaks(&self) {
        let imp = self.imp();
        if !imp.audio_started.get()
            || !imp.remote_missing.get()
            || imp.remote_pending.get()
            || imp.remote_retries.get() >= 6
            || imp.remote_retry.borrow().is_some()
        {
            return;
        }
        let id = imp.song.borrow().as_ref().and_then(Song::remote_id);
        if let Some(id) = id {
            // Playback can start before download verification stores peaks.
            // At most six local API retries (1/2/4/8/16/32 seconds), never
            // a peer search, another audio download, or unbounded polling.
            let delay = std::time::Duration::from_secs(1 << imp.remote_retries.get());
            imp.remote_retries.set(imp.remote_retries.get() + 1);
            let generation = imp.generation.get();
            let retry = glib::timeout_add_local_once(
                delay,
                clone!(
                    #[weak(rename_to = this)]
                    self,
                    move || {
                        if this.imp().generation.get() != generation {
                            return;
                        }
                        this.imp().remote_retry.take();
                        this.fetch_remote_peaks(id);
                    }
                ),
            );
            imp.remote_retry.replace(Some(retry));
        }
    }

    fn fetch_remote_peaks(&self, id: i64) {
        self.imp().peaks.replace(None);

        let Some(client) = library::client() else {
            self.notify("has-peaks");
            return;
        };
        self.imp().remote_pending.set(true);
        let generation = self.imp().generation.get();

        glib::MainContext::default().spawn_local(clone!(
            #[weak(rename_to = this)]
            self,
            async move {
                let peaks = client.peaks(id).await;

                // The song may have changed while we were waiting
                let current = this
                    .imp()
                    .song
                    .borrow()
                    .as_ref()
                    .and_then(|s| s.remote_id());
                if current != Some(id) || this.imp().generation.get() != generation {
                    return;
                }
                this.imp().remote_pending.set(false);

                match peaks {
                    Ok(peaks) => {
                        this.imp().remote_missing.set(false);
                        let peaks = peaks
                            .into_iter()
                            .map(|(l, r)| (f64::from(l), f64::from(r)))
                            .collect();
                        this.imp().peaks.replace(Some(peaks));
                        this.save_peaks();
                    }
                    Err(err) => {
                        this.imp()
                            .remote_missing
                            .set(matches!(err, library::ClientError::NotFound));
                        warn!("Unable to fetch the waveform: {err}");
                        this.notify("has-peaks");
                        this.retry_remote_peaks();
                    }
                }
            }
        ));
    }

    fn generate_peaks(&self) {
        if let Some((pipeline, _bus_watch)) = self.imp().pipeline.take() {
            // Stop any running pipeline, and ensure that we have nothing to
            // report
            self.imp().peaks.replace(None);
            pipeline.send_event(gst::event::Eos::new());
            match pipeline.set_state(gst::State::Null) {
                Ok(_) => {}
                Err(err) => warn!("Unable to set existing pipeline to Null state: {}", err),
            }
        }

        let song = match self.imp().song.borrow().as_ref() {
            Some(s) => s.clone(),
            None => {
                self.imp().peaks.replace(None);
                self.notify("has-peaks");
                return;
            }
        };

        // The shared library computes waveforms when a song is uploaded, so
        // there is no need to download the whole song a second time
        if let Some(id) = song.remote_id() {
            self.fetch_remote_peaks(id);
            return;
        }

        // Reset the peaks vector
        let peaks: Vec<(f64, f64)> = Vec::new();
        self.imp().peaks.replace(Some(peaks));

        let pipeline_str = "uridecodebin name=uridecodebin ! audioconvert ! audio/x-raw,channels=2 ! level name=level interval=250000000 ! fakesink name=faked";
        let pipeline = match gst::parse::launch(pipeline_str) {
            Ok(pipeline) => pipeline,
            Err(err) => {
                warn!("Unable to generate the waveform: {}", err);
                self.imp().peaks.replace(None);
                self.notify("has-peaks");
                return;
            }
        };

        let uridecodebin = pipeline
            .downcast_ref::<gst::Bin>()
            .unwrap()
            .by_name("uridecodebin")
            .unwrap();
        uridecodebin.set_property("uri", song.uri());

        let fakesink = pipeline
            .downcast_ref::<gst::Bin>()
            .unwrap()
            .by_name("faked")
            .unwrap();
        fakesink.set_property("qos", false);
        fakesink.set_property("sync", false);

        let bus = pipeline
            .bus()
            .expect("Pipeline without bus. Shouldn't happen!");

        debug!("Adding bus watch");
        let bus_watch = bus
            .add_watch_local(clone!(
                #[weak(rename_to = this)]
                self,
                #[weak]
                pipeline,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move |_, msg| {
                    use gst::MessageView;

                    match msg.view() {
                        MessageView::Eos(..) => {
                            debug!("End of waveform stream");
                            pipeline
                                .set_state(gst::State::Null)
                                .expect("Unable to set 'null' state");
                            // We're done
                            this.imp().pipeline.replace(None);
                            this.save_peaks();
                            return glib::ControlFlow::Break;
                        }
                        MessageView::Error(err) => {
                            warn!("Pipeline error: {:?}", err);
                            pipeline
                                .set_state(gst::State::Null)
                                .expect("Unable to set 'null' state");
                            // We're done
                            this.imp().pipeline.replace(None);
                            // Missing codecs must not leave a permanently empty
                            // cache that survives an application upgrade.
                            this.imp().peaks.replace(None);
                            this.notify("has-peaks");
                            return glib::ControlFlow::Break;
                        }
                        MessageView::Element(element) => {
                            if let Some(s) = element.structure() {
                                if s.has_name("level") {
                                    let peaks_array = s.get::<&glib::ValueArray>("peak").unwrap();
                                    let v1 = peaks_array[0].get::<f64>().unwrap();
                                    let v2 = peaks_array[1].get::<f64>().unwrap();
                                    // Normalize peaks between 0 and 1
                                    let peak1 = f64::powf(10.0, v1 / 20.0);
                                    let peak2 = f64::powf(10.0, v2 / 20.0);
                                    if let Some(ref mut peaks) = *this.imp().peaks.borrow_mut() {
                                        peaks.push((peak1, peak2));
                                    }
                                }
                            }
                        }
                        _ => (),
                    };

                    glib::ControlFlow::Continue
                }
            ))
            .expect("failed to add bus watch");

        match pipeline.set_state(gst::State::Playing) {
            Ok(_) => {
                self.imp().pipeline.replace(Some((pipeline, bus_watch)));
            }
            Err(err) => {
                warn!("Unable to generate the waveform: {}", err);
                pipeline
                    .set_state(gst::State::Null)
                    .expect("Pipeline reset failed");
                self.imp().peaks.replace(None);
                self.notify("has-peaks");
            }
        };
    }
}
