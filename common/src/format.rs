// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::{Deserialize, Serialize};

/// Number of leading bytes [`AudioFormat::sniff`] needs to make a decision.
///
/// Ogg needs the most: a 27 byte page header, up to 255 lacing values, and
/// the first bytes of the identification packet.
pub const SNIFF_LEN: usize = 512;

/// The audio formats accepted for upload.
///
/// Every format in this list can be fully demuxed *and* decoded by the
/// server's validator; formats it can only partially understand (Opus,
/// Matroska, AIFF, ...) are deliberately left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Flac,
    Mp3,
    /// Audio-only MPEG-4 (AAC or ALAC), uploaded as `.m4a`. The `.mp4`
    /// extension is refused: it names video files
    Mp4,
    /// Ogg container with a single Vorbis stream
    Ogg,
    /// RIFF WAVE with integer or float PCM
    Wav,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 5] = [
        AudioFormat::Flac,
        AudioFormat::Mp3,
        AudioFormat::Mp4,
        AudioFormat::Ogg,
        AudioFormat::Wav,
    ];

    /// File extensions accepted for each format, lowercase and without the dot.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            AudioFormat::Flac => &["flac"],
            AudioFormat::Mp3 => &["mp3"],
            AudioFormat::Mp4 => &["m4a"],
            AudioFormat::Ogg => &["ogg", "oga"],
            AudioFormat::Wav => &["wav"],
        }
    }

    /// Every accepted extension, for file chooser filters.
    pub fn all_extensions() -> impl Iterator<Item = &'static str> {
        Self::ALL
            .iter()
            .flat_map(|f| f.extensions().iter().copied())
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        let ext = ext.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|f| f.extensions().contains(&ext.as_str()))
    }

    /// Looks up the format from the extension of a file name.
    pub fn from_file_name(name: &str) -> Option<Self> {
        let (_, ext) = name.rsplit_once('.')?;
        Self::from_extension(ext)
    }

    /// `name` without an accepted audio extension, if it ends with one:
    /// "A.mp3" becomes "A". Used to show titles that are really file names.
    pub fn strip_extension(name: &str) -> &str {
        match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.trim().is_empty() && Self::from_extension(ext).is_some() => {
                stem
            }
            _ => name,
        }
    }

    /// The extension the server stores files with.
    pub fn canonical_extension(self) -> &'static str {
        match self {
            AudioFormat::Flac => "flac",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Mp4 => "m4a",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Wav => "wav",
        }
    }

    pub fn mime_type(self) -> &'static str {
        match self {
            AudioFormat::Flac => "audio/flac",
            AudioFormat::Mp3 => "audio/mpeg",
            AudioFormat::Mp4 => "audio/mp4",
            AudioFormat::Ogg => "audio/ogg",
            AudioFormat::Wav => "audio/wav",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            AudioFormat::Flac => "FLAC",
            AudioFormat::Mp3 => "MP3",
            AudioFormat::Mp4 => "MPEG-4 audio",
            AudioFormat::Ogg => "Ogg Vorbis",
            AudioFormat::Wav => "WAV",
        }
    }

    /// Identifies the format from the first bytes of a file.
    ///
    /// This only looks at the container signature and is never a sufficient
    /// check on its own: anything can be prefixed with four magic bytes. The
    /// server follows it up with a full demux and decode.
    pub fn sniff(header: &[u8]) -> Option<Self> {
        if header.starts_with(b"fLaC") {
            return Some(AudioFormat::Flac);
        }

        if header.len() >= 12 && &header[0..4] == b"RIFF" && &header[8..12] == b"WAVE" {
            return Some(AudioFormat::Wav);
        }

        if is_mp4(header) {
            return Some(AudioFormat::Mp4);
        }

        if is_ogg_vorbis(header) {
            return Some(AudioFormat::Ogg);
        }

        if is_mp3(header) {
            return Some(AudioFormat::Mp3);
        }

        None
    }
}

// Major brands of audio-capable ISO base media files. Anything else (HEIF
// images, QuickTime, 3GPP, ...) is rejected before parsing.
const MP4_BRANDS: &[&[u8; 4]] = &[
    b"M4A ", b"M4B ", b"mp41", b"mp42", b"isom", b"iso2", b"iso4", b"iso5", b"iso6", b"dash",
];

fn is_mp4(h: &[u8]) -> bool {
    if h.len() < 12 || &h[4..8] != b"ftyp" {
        return false;
    }
    let size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]);
    if !(16..=256).contains(&size) {
        return false;
    }
    MP4_BRANDS.iter().any(|b| &h[8..12] == *b)
}

fn ogg_first_packet(h: &[u8]) -> Option<&[u8]> {
    // Capture pattern, stream structure version 0, beginning-of-stream flag
    if h.len() < 27 || &h[0..4] != b"OggS" || h[4] != 0 || h[5] & 0x02 == 0 {
        return None;
    }
    let n_segments = h[26] as usize;
    h.get(27 + n_segments..)
}

fn is_ogg_vorbis(h: &[u8]) -> bool {
    matches!(ogg_first_packet(h), Some(p) if p.starts_with(b"\x01vorbis"))
}

fn is_mp3(h: &[u8]) -> bool {
    if h.len() >= 10 && &h[0..3] == b"ID3" {
        // ID3v2.2 - v2.4, revision 0; sizes are syncsafe so no byte may
        // have its top bit set
        return (2..=4).contains(&h[3]) && h[4] == 0 && h[6..10].iter().all(|b| b & 0x80 == 0);
    }
    is_mpeg_layer3_header(h)
}

/// Checks for a plausible MPEG audio Layer III frame header.
fn is_mpeg_layer3_header(h: &[u8]) -> bool {
    if h.len() < 4 || h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return false;
    }
    let version = (h[1] >> 3) & 0x03; // 01 is reserved
    let layer = (h[1] >> 1) & 0x03; // 01 is Layer III
    let bitrate = h[2] >> 4; // 0000 is free format, 1111 is invalid
    let sample_rate = (h[2] >> 2) & 0x03; // 11 is reserved
    version != 0x01 && layer == 0x01 && bitrate != 0x00 && bitrate != 0x0F && sample_rate != 0x03
}

/// Gives a human readable guess for what a rejected file actually is, so a
/// renamed executable or archive gets a clearer error than "not audio".
pub fn describe_non_audio(h: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| h.starts_with(sig);
    if starts(b"MZ") {
        Some("a Windows executable")
    } else if starts(b"\x7fELF") {
        Some("a Linux executable")
    } else if starts(&[0xCF, 0xFA, 0xED, 0xFE]) || starts(&[0xCA, 0xFE, 0xBA, 0xBE]) {
        Some("a macOS executable")
    } else if starts(b"#!") {
        Some("a script")
    } else if starts(b"PK\x03\x04") || starts(b"Rar!") || starts(b"7z\xBC\xAF") {
        Some("an archive")
    } else if starts(b"%PDF") {
        Some("a PDF document")
    } else if starts(&[0xD0, 0xCF, 0x11, 0xE0]) {
        Some("an Office document")
    } else if ogg_first_packet(h).is_some_and(|p| p.starts_with(b"OpusHead")) {
        Some("Opus audio, which is not supported")
    } else if ogg_first_packet(h).is_some() {
        Some("an Ogg file that does not contain Vorbis audio")
    } else if h.len() >= 8 && &h[4..8] == b"ftyp" {
        Some("an MPEG-4 file of an unsupported kind")
    } else if h.len() >= 4 && &h[0..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        Some("a Matroska/WebM file, which is not supported")
    } else {
        let text = h.iter().take(64).skip_while(|b| b.is_ascii_whitespace());
        let text: Vec<u8> = text.copied().collect();
        if text.starts_with(b"<") {
            Some("an HTML or XML document")
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(AudioFormat::from_extension("FLAC"), Some(AudioFormat::Flac));
        assert_eq!(AudioFormat::from_extension("m4a"), Some(AudioFormat::Mp4));
        assert_eq!(AudioFormat::from_extension("mp4"), None);
        assert_eq!(AudioFormat::from_extension("exe"), None);
        assert_eq!(AudioFormat::from_extension("opus"), None);
        assert_eq!(
            AudioFormat::from_file_name("song.final.m4a"),
            Some(AudioFormat::Mp4)
        );
        assert_eq!(AudioFormat::from_file_name("flac"), None);
        assert_eq!(AudioFormat::from_file_name("song.flac.exe"), None);
    }

    #[test]
    fn sniff_signatures() {
        assert_eq!(
            AudioFormat::sniff(b"fLaC\0\0\0\x22"),
            Some(AudioFormat::Flac)
        );
        assert_eq!(
            AudioFormat::sniff(b"RIFF\x24\0\0\0WAVEfmt "),
            Some(AudioFormat::Wav)
        );
        assert_eq!(
            AudioFormat::sniff(b"\0\0\0\x20ftypM4A \0\0\0\0"),
            Some(AudioFormat::Mp4)
        );
        assert_eq!(AudioFormat::sniff(b"\0\0\0\x20ftypheic\0\0\0\0"), None);
        assert_eq!(
            AudioFormat::sniff(b"ID3\x04\0\0\0\0\x01\x00"),
            Some(AudioFormat::Mp3)
        );
        // MPEG-1 Layer III, 128 kbit/s, 44.1 kHz
        assert_eq!(
            AudioFormat::sniff(&[0xFF, 0xFB, 0x90, 0x64]),
            Some(AudioFormat::Mp3)
        );
        // MPEG-1 Layer II is not accepted
        assert_eq!(AudioFormat::sniff(&[0xFF, 0xFD, 0x90, 0x64]), None);
    }

    fn ogg_page(first_packet: &[u8]) -> Vec<u8> {
        let mut page = b"OggS\0\x02".to_vec();
        page.extend_from_slice(&[0; 20]);
        page.push(1);
        page.push(first_packet.len() as u8);
        page.extend_from_slice(first_packet);
        page
    }

    #[test]
    fn sniff_ogg() {
        let vorbis = ogg_page(b"\x01vorbis\0\0\0\0");
        assert_eq!(AudioFormat::sniff(&vorbis), Some(AudioFormat::Ogg));

        let opus = ogg_page(b"OpusHead\x01\x02");
        assert_eq!(AudioFormat::sniff(&opus), None);
        assert_eq!(
            describe_non_audio(&opus),
            Some("Opus audio, which is not supported")
        );
    }

    #[test]
    fn disguised_files() {
        assert_eq!(AudioFormat::sniff(b"MZ\x90\0\x03\0\0\0"), None);
        assert_eq!(
            describe_non_audio(b"MZ\x90\0"),
            Some("a Windows executable")
        );
        assert_eq!(
            describe_non_audio(b"\x7fELF\x02"),
            Some("a Linux executable")
        );
        assert_eq!(describe_non_audio(b"PK\x03\x04"), Some("an archive"));
        assert_eq!(
            describe_non_audio(b"  <!DOCTYPE html>"),
            Some("an HTML or XML document")
        );
        assert_eq!(describe_non_audio(b"random"), None);
    }
}
