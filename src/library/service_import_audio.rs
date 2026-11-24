// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline conversion of an adapter's bounded local file; no URI/network source.
use gst::prelude::*;
use std::convert::TryFrom;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(super) fn convert(root: &Path, source: &Path, cancelled: &AtomicBool) -> Result<PathBuf, ()> {
    const LIMIT: u64 = 512 * 1024 * 1024;
    let root = root.canonicalize().map_err(|_| ())?;
    let source = source.canonicalize().map_err(|_| ())?;
    let name = source.file_stem().and_then(|s| s.to_str()).ok_or(())?;
    if source.parent() != Some(root.as_path())
        || source.extension().and_then(|s| s.to_str()) != Some("source")
        || name.len() != 64
        || !name.bytes().all(|b| b.is_ascii_hexdigit())
        || !source.is_file()
        || source.metadata().map_err(|_| ())?.len() > LIMIT
    {
        return Err(());
    }
    let metadata_path = source.with_extension("tags.json");
    if metadata_path.metadata().map_err(|_| ())?.len() > 8192 {
        return Err(());
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&metadata_path).map_err(|_| ())?).map_err(|_| ())?;
    let target = source.with_extension("flac");
    let partial = source.with_extension("converting");
    let pipeline = gst::parse::launch("filesrc name=input ! decodebin ! audioconvert ! audioresample ! audio/x-raw,rate=44100,channels=2 ! flacenc name=encoder ! filesink name=output")
        .map_err(|_| ())?.downcast::<gst::Pipeline>().map_err(|_| ())?;
    pipeline
        .by_name("input")
        .ok_or(())?
        .set_property("location", source.to_str().ok_or(())?);
    pipeline
        .by_name("output")
        .ok_or(())?
        .set_property("location", partial.to_str().ok_or(())?);
    let encoder = pipeline
        .by_name("encoder")
        .ok_or(())?
        .dynamic_cast::<gst::TagSetter>()
        .map_err(|_| ())?;
    let mut tags = gst::TagList::new();
    {
        let tags = tags.get_mut().ok_or(())?;
        if let Some(v) = metadata["title"].as_str() {
            tags.add::<gst::tags::Title>(&v, gst::TagMergeMode::Replace);
        }
        if let Some(v) = metadata["artist"].as_str() {
            tags.add::<gst::tags::Artist>(&v, gst::TagMergeMode::Replace);
        }
        if let Some(v) = metadata["album"].as_str() {
            tags.add::<gst::tags::Album>(&v, gst::TagMergeMode::Replace);
        }
        if let Some(v) = metadata["album_artist"].as_str() {
            tags.add::<gst::tags::AlbumArtist>(&v, gst::TagMergeMode::Replace);
        }
        if let Some(v) = metadata["track_number"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
        {
            tags.add::<gst::tags::TrackNumber>(&v, gst::TagMergeMode::Replace);
        }
    }
    encoder.merge_tags(&tags, gst::TagMergeMode::ReplaceAll);
    let result = (|| {
        pipeline.set_state(gst::State::Playing).map_err(|_| ())?;
        let bus = pipeline.bus().ok_or(())?;
        let deadline = Instant::now() + Duration::from_secs(600);
        loop {
            if cancelled.load(Ordering::Acquire)
                || Instant::now() >= deadline
                || partial.metadata().map(|m| m.len() > LIMIT).unwrap_or(false)
                || pipeline
                    .query_position::<gst::ClockTime>()
                    .is_some_and(|t| t.seconds() > 10800)
            {
                return Err(());
            }
            if let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) {
                match msg.view() {
                    gst::MessageView::Eos(_) => return Ok(()),
                    gst::MessageView::Error(_) => return Err(()),
                    _ => (),
                }
            }
        }
    })();
    let stopped = pipeline.set_state(gst::State::Null).map_err(|_| ());
    let result = result.and(stopped.map(|_| ())).and_then(|_| {
        let size = partial.metadata().map_err(|_| ())?.len();
        if size == 0 || size > LIMIT || cancelled.load(Ordering::Acquire) {
            return Err(());
        }
        std::fs::rename(&partial, &target).map_err(|_| ())
    });
    let _ = std::fs::remove_file(partial);
    let _ = std::fs::remove_file(source);
    let _ = std::fs::remove_file(metadata_path);
    result.map(|_| target)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_audio_conversion_tags_cancellation_and_path_boundary() {
        use lofty::{file::TaggedFileExt, tag::Accessor};
        gst::init().unwrap();
        let root = std::env::temp_dir().join(format!(
            "hopea-import-convert-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join(format!("{}.source", "a".repeat(64)));
        let pipeline = gst::parse::launch(
            "audiotestsrc num-buffers=10 ! audioconvert ! wavenc ! filesink name=output",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        pipeline
            .by_name("output")
            .unwrap()
            .set_property("location", source.to_str().unwrap());
        pipeline.set_state(gst::State::Playing).unwrap();
        let message = pipeline
            .bus()
            .unwrap()
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(10),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .unwrap();
        assert_eq!(message.type_(), gst::MessageType::Eos);
        pipeline.set_state(gst::State::Null).unwrap();
        let audio = std::fs::read(&source).unwrap();
        let metadata = source.with_extension("tags.json");
        std::fs::write(
            &metadata,
            r#"{"title":"Test song","artist":"Test artist","album":"Album"}"#,
        )
        .unwrap();
        let target = convert(&root, &source, &AtomicBool::new(false)).unwrap();
        let file = lofty::read_from_path(&target).unwrap();
        assert_eq!(
            file.primary_tag().unwrap().title().as_deref(),
            Some("Test song")
        );
        assert!(!source.exists());
        std::fs::remove_file(&target).unwrap();
        std::fs::write(&source, audio).unwrap();
        std::fs::write(&metadata, "{}").unwrap();
        assert!(convert(&root, &source, &AtomicBool::new(true)).is_err());
        assert!(!target.exists());
        assert!(!source.with_extension("converting").exists());
        assert!(convert(&root.join("other"), &source, &AtomicBool::new(false)).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
