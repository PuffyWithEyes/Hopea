// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keep Android document-provider selections in private durable storage. A
//! content URI is not a filesystem path and its grant may end on app restart.
use gtk::{gio, glib, prelude::*};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

struct Pending(PathBuf);
impl Drop for Pending {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub async fn local_audio(file: &gio::File) -> Result<gio::File, glib::Error> {
    if file.path().is_some() {
        return Ok(file.clone());
    }
    let invalid = || {
        glib::Error::new(
            gio::IOErrorEnum::InvalidData,
            "Unsupported or oversized audio document",
        )
    };
    if !file.uri().starts_with("content://") {
        return Err(invalid());
    }
    let input = file.read_future(glib::Priority::DEFAULT).await?;
    let directory = crate::paths::data_dir().join("imported-documents");
    copy_audio(input.upcast_ref(), &directory, 512 * 1024 * 1024).await
}

async fn copy_audio(
    input: &gio::InputStream,
    directory: &std::path::Path,
    limit: u64,
) -> Result<gio::File, glib::Error> {
    let invalid = || {
        glib::Error::new(
            gio::IOErrorEnum::InvalidData,
            "Unsupported or oversized audio document",
        )
    };
    if glib::mkdir_with_parents(directory, 0o700) != 0 {
        return Err(invalid());
    }
    let pending = Pending(directory.join(format!("{}.part", glib::uuid_string_random())));
    let output_file = gio::File::for_path(&pending.0);
    let output = output_file
        .create_future(gio::FileCreateFlags::PRIVATE, glib::Priority::DEFAULT)
        .await?;
    let mut hash = Sha256::new();
    let mut header = Vec::new();
    let mut size = 0_u64;
    loop {
        let chunk = input
            .read_bytes_future(64 * 1024, glib::Priority::DEFAULT)
            .await?;
        if chunk.is_empty() {
            break;
        }
        size = size.checked_add(chunk.len() as u64).ok_or_else(invalid)?;
        if size > limit {
            return Err(invalid());
        }
        header.extend_from_slice(
            &chunk[..chunk
                .len()
                .min(hopea_common::SNIFF_LEN.saturating_sub(header.len()))],
        );
        if header.len() == hopea_common::SNIFF_LEN
            && hopea_common::AudioFormat::sniff(&header).is_none()
        {
            return Err(invalid());
        }
        hash.update(&chunk);
        let (_, _, error) = output
            .write_all_future(chunk.to_vec(), glib::Priority::DEFAULT)
            .await
            .map_err(|(_, e)| e)?;
        if let Some(error) = error {
            return Err(error);
        }
    }
    output.close_future(glib::Priority::DEFAULT).await?;
    let format = hopea_common::AudioFormat::sniff(&header).ok_or_else(invalid)?;
    let final_path = directory.join(format!(
        "{}.{}",
        hex::encode(hash.finalize()),
        format.canonical_extension()
    ));
    if !final_path.exists() {
        std::fs::rename(&pending.0, &final_path).map_err(|_| invalid())?;
    }
    Ok(gio::File::for_path(final_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn documents_enforce_actual_byte_limit_and_reuse_identical_files() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let directory = std::env::temp_dir()
                        .join(format!("hopea-doc-test-{}", glib::uuid_string_random()));
                    let audio = include_bytes!("../node/tests/fixtures/tone.wav");
                    let stream = || {
                        gio::MemoryInputStream::from_bytes(&glib::Bytes::from_static(audio))
                            .upcast::<gio::InputStream>()
                    };
                    assert!(copy_audio(&stream(), &directory, 512).await.is_err());
                    assert_eq!(
                        std::fs::read_dir(&directory).unwrap().count(),
                        0,
                        "partial oversized files must be removed"
                    );
                    let garbage =
                        gio::MemoryInputStream::from_bytes(&glib::Bytes::from_static(b"not audio"));
                    assert!(copy_audio(garbage.upcast_ref(), &directory, 1024)
                        .await
                        .is_err());
                    let first = copy_audio(&stream(), &directory, audio.len() as u64)
                        .await
                        .unwrap();
                    let again = copy_audio(&stream(), &directory, audio.len() as u64)
                        .await
                        .unwrap();
                    assert_eq!(first.path(), again.path());
                    assert_eq!(std::fs::read(first.path().unwrap()).unwrap(), audio);
                    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
                    std::fs::remove_dir_all(directory).unwrap();
                })
            })
            .unwrap();
    }
}
