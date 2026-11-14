// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cover art is never passed through as uploaded: it is decoded (PNG and
//! JPEG only, with dimension and allocation limits), scaled down and
//! re-encoded as a baseline JPEG. Clients only ever parse images produced
//! by this encoder.

use std::io::Cursor;

use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, ImageFormat, ImageReader, Limits};

const MAX_DIMENSION: u32 = 8192;
const MAX_ALLOC: u64 = 64 * 1024 * 1024;
const OUTPUT_SIZE: u32 = 1024;
const OUTPUT_QUALITY: u8 = 85;

pub fn reencode(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() > 16 * 1024 * 1024 {
        return None;
    }
    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return None;
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC);
    // Check the product as well as each dimension. Compressed byte size says
    // nothing about the memory needed for pixels. Use u64 before multiplying.
    let mut dimensions = ImageReader::with_format(Cursor::new(data), reader.format()?);
    // Header parsing may itself decompress PNG text/ICC metadata.
    dimensions.limits(limits.clone());
    let (width, height) = dimensions.into_dimensions().ok()?;
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_ALLOC / 4
    {
        return None;
    }

    reader.limits(limits);

    let image = match reader.decode() {
        Ok(image) => image,
        Err(err) => {
            tracing::debug!("Discarding undecodable cover art: {err}");
            return None;
        }
    };

    let image = if image.width() > OUTPUT_SIZE || image.height() > OUTPUT_SIZE {
        image.resize(OUTPUT_SIZE, OUTPUT_SIZE, FilterType::Triangle)
    } else {
        image
    };

    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, OUTPUT_QUALITY)
        .encode_image(&image.to_rgb8())
        .ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut image = b"\x89PNG\r\n\x1a\n".to_vec();
        image.extend(13u32.to_be_bytes());
        let mut chunk = b"IHDR".to_vec();
        chunk.extend(width.to_be_bytes());
        chunk.extend(height.to_be_bytes());
        chunk.extend([8, 2, 0, 0, 0]);
        let mut crc = !0u32;
        for b in &chunk {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        image.extend(chunk);
        image.extend((!crc).to_be_bytes());
        image.extend(b"\0\0\0\0IEND\xaeB`\x82");
        image
    }
    #[test]
    fn tiny_files_cannot_claim_gigantic_pixel_buffers() {
        for (width, height) in [(u32::MAX, u32::MAX), (8192, 8192), (65536, 65536), (0, 1)] {
            let data = png_header(width, height);
            assert!(data.len() < 100);
            assert!(reencode(&data).is_none());
        }
        let mut data = Cursor::new(Vec::new());
        image::RgbImage::new(32, 32)
            .write_to(&mut data, ImageFormat::Png)
            .unwrap();
        assert!(reencode(data.get_ref()).is_some());
    }
}
