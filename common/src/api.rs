// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSON types and paths of the shared library HTTP API.
//!
//! This is the API between the player and the node built into it (see
//! `node/`). Every endpoint requires an `Authorization: Bearer <token>`
//! header.

use serde::{Deserialize, Serialize};

use crate::AudioFormat;

pub const API_PREFIX: &str = "/api/v1";

/// Request header carrying the original file name of an upload,
/// percent-encoded. Only its extension is used for validation.
pub const FILENAME_HEADER: &str = "x-anysik-filename";

/// Request header of an upload naming its cover art: the `cover` returned
/// by [`covers_path`]. Replaces any picture in the file. An owner can later
/// PUT another pending cover to [`cover_path`] without uploading audio again.
pub const COVER_HEADER: &str = "x-anysik-cover";

/// Like [`COVER_HEADER`], but only used when the file has no picture of
/// its own, as for a folder's `cover.jpg`.
pub const FALLBACK_COVER_HEADER: &str = "x-anysik-fallback-cover";

/// Where a picture is sent, as a PNG or JPEG request body, before the
/// uploads it is for. Answers with a [`PendingCover`].
pub fn covers_path() -> String {
    format!("{API_PREFIX}/covers")
}

/// A picture ready to be named by [`COVER_HEADER`] in uploads that follow
/// soon after.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingCover {
    pub cover: String,
}

pub fn me_path() -> String {
    format!("{API_PREFIX}/me")
}

pub fn tracks_path() -> String {
    format!("{API_PREFIX}/tracks")
}

pub fn track_path(id: i64) -> String {
    format!("{API_PREFIX}/tracks/{id}")
}

pub fn audio_path(id: i64) -> String {
    format!("{API_PREFIX}/tracks/{id}/audio")
}

pub fn cover_path(id: i64) -> String {
    format!("{API_PREFIX}/tracks/{id}/cover")
}

pub fn peaks_path(id: i64) -> String {
    format!("{API_PREFIX}/tracks/{id}/peaks")
}

/// The file itself, to save it: only once this node has it in full and
/// verified.
pub fn file_path(id: i64) -> String {
    format!("{API_PREFIX}/tracks/{id}/file")
}

/// The tracks of an album: `?album=…&artist=…&network=true`.
pub fn album_path() -> String {
    format!("{API_PREFIX}/album")
}

/// `GET` the speed limits in effect, `PUT` new ones ([`SpeedLimits`]).
pub fn limits_path() -> String {
    format!("{API_PREFIX}/limits")
}

/// How many times slower than downloading sharing may be limited to.
pub const MAX_SHARING_SLOWDOWN: u32 = 7;
/// The lowest limit accepted, in bytes per second. BitTorrent moves data in
/// 16 KiB blocks, so anything lower would stall transfers.
pub const MIN_LIMIT_BPS: u32 = 32 * 1024;

/// Speed limits for downloading and sharing music, in bytes per second;
/// `None` means no limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeedLimits {
    pub download: Option<u32>,
    pub upload: Option<u32>,
}

impl Default for SpeedLimits {
    fn default() -> Self {
        Self {
            download: Some(20 * 1024 * 1024),
            upload: Some(10 * 1024 * 1024),
        }
    }
}

impl SpeedLimits {
    /// Temporary gaming cap. Never increases an existing fair user limit,
    /// including when upload is particularly low. Does not alter preferences.
    pub fn during_game(self) -> Self {
        let base = self.fair();
        let upload = base.upload.unwrap_or(u32::MAX).min(128 * 1024);
        let download = base
            .download
            .unwrap_or(u32::MAX)
            .min(512 * 1024)
            .min(upload.saturating_mul(MAX_SHARING_SLOWDOWN));
        Self {
            download: Some(download),
            upload: Some(upload),
        }
        .fair()
    }

    /// The limits made fair: sharing may be at most
    /// [`MAX_SHARING_SLOWDOWN`] times slower than downloading. So sharing
    /// can only be limited when downloading is, and never below that share
    /// of the download limit. No limit is below [`MIN_LIMIT_BPS`].
    pub fn fair(self) -> Self {
        let download = self.download.map(|d| d.max(MIN_LIMIT_BPS));
        let upload = match (download, self.upload) {
            (Some(d), Some(u)) => Some(u.max(d.div_ceil(MAX_SHARING_SLOWDOWN)).max(MIN_LIMIT_BPS)),
            _ => None,
        };
        Self { download, upload }
    }
}

/// `PUT` adds a track to the favorites, `DELETE` removes it.
pub fn favorite_path(id: i64) -> String {
    format!("{API_PREFIX}/tracks/{id}/favorite")
}

pub fn favorites_path() -> String {
    format!("{API_PREFIX}/favorites")
}

/// The "For you" feed; `?refresh=true` searches the network again.
pub fn recommendations_path() -> String {
    format!("{API_PREFIX}/recommendations")
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub duration_ms: u64,
    pub format: AudioFormat,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub size: u64,
    pub sha256: String,
    pub has_cover: bool,
    /// Who shared the track: the name they chose, if any, and a short form
    /// of their key. Names are not verified; the key part is.
    pub uploader: String,
    /// Unix timestamp, in seconds
    pub uploaded_at: i64,
    /// Shared by this node
    #[serde(default)]
    pub mine: bool,
    /// The whole file is on this computer
    #[serde(default)]
    pub local: bool,
    /// In the listener's favorites, which are kept on this computer
    #[serde(default)]
    pub favorite: bool,
    /// Computers known to share the track, this one included when it has
    /// the file: how popular the track is
    #[serde(default)]
    pub sources: u32,
}

/// The order of search results.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchOrder {
    /// Closest to the query first; of equally close ones, the most shared
    #[default]
    BestMatch,
    /// The most shared first
    Popular,
}

impl SearchOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            SearchOrder::BestMatch => "best_match",
            SearchOrder::Popular => "popular",
        }
    }
}

/// Fewest letters and digits a shared song's title and artist may have
/// together: less than that, and nobody finds it.
pub const MIN_NAME_CHARS: usize = 5;

/// Whether a song's name is enough for others to find it (see
/// [`MIN_NAME_CHARS`]). A file extension left in the title doesn't count.
pub fn name_is_findable(title: &str, artist: Option<&str>) -> bool {
    let count = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).count();
    count(AudioFormat::strip_extension(title)) + artist.map_or(0, count) >= MIN_NAME_CHARS
}

/// How a track's file is coming along on this computer, and how it is
/// shared from here.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Transfer {
    /// Share of the file on this computer, from 0 to 1
    pub progress: f32,
    /// Computers connected for this track right now
    pub peers: u32,
    pub download_bps: u64,
    pub upload_bps: u64,
    /// Sent to others since the app started
    pub uploaded_bytes: u64,
}

/// `POST` starts downloading a track ahead of time, as the song before it
/// plays, so that it plays without waiting.
pub fn prefetch_path(id: i64) -> String {
    format!("{API_PREFIX}/tracks/{id}/prefetch")
}

pub fn transfer_path(id: i64) -> String {
    format!("{API_PREFIX}/tracks/{id}/transfer")
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackPage {
    pub tracks: Vec<Track>,
    pub total: u64,
    /// Other nodes that answered the network search, if one was made
    #[serde(default)]
    pub nodes_answered: u32,
    /// How the network search went, if one was made
    #[serde(default)]
    pub report: Option<SearchReport>,
}

/// How a network search went, to tell the listener what stopped it, if
/// something did, rather than show nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchReport {
    pub dht: DhtState,
    /// Computers asked: those the DHT named for the query's words, and those
    /// always asked (`library-peers`)
    pub nodes_asked: u32,
    pub nodes_answered: u32,
    /// Why most of the computers that didn't answer didn't
    pub failure: Option<SearchFailure>,
    /// The search ran out of time before every computer answered
    pub timed_out: bool,
}

/// Whether the BitTorrent DHT, through which computers find each other,
/// could be used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DhtState {
    /// Turned off on this node
    #[default]
    Off,
    /// No other DHT node answered, as when UDP is blocked
    NoContact,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchFailure {
    /// No answer in time
    Timeout,
    /// The connection was refused or the computer couldn't be reached
    Unreachable,
    /// The answer wasn't a search answer
    BadAnswer,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Me {
    /// How this node's tracks are labelled for others
    pub name: String,
    pub used_bytes: u64,
    pub quota_bytes: u64,
    pub max_upload_bytes: u64,
    /// Whether other people can find the music shared from here, and how
    #[serde(default)]
    pub reachability: Reachability,
}

/// How other nodes reach this one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    /// Still finding out
    #[default]
    Checking,
    /// Directly: an open or forwarded port (possibly through UPnP), or a
    /// public address
    Direct,
    /// Directly, but only over IPv6
    Ipv6,
    /// Through another node that relays searches and files
    Relay,
    /// Not at all: searching and listening work, but nobody can find the
    /// music shared from here
    Unreachable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UploadResponse {
    pub track: Track,
    /// The exact same audio was already in the library
    pub duplicate: bool,
}

/// Waveform peaks, one `(left, right)` pair per 250 ms, normalized to 0..1.
pub type Peaks = Vec<(f32, f32)>;

/// Body of every non-2xx response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    /// Stable machine readable code, e.g. `not_audio`
    pub error: String,
    /// Human readable explanation, safe to show to the user
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_serializes_lowercase() {
        let s = serde_json::to_string(&AudioFormat::Mp4).unwrap();
        assert_eq!(s, "\"mp4\"");
    }

    #[test]
    fn gaming_caps_preserve_lower_limits_and_fairness() {
        for download in [
            None,
            Some(1),
            Some(32 * 1024),
            Some(256 * 1024),
            Some(u32::MAX),
        ] {
            for upload in [
                None,
                Some(1),
                Some(32 * 1024),
                Some(64 * 1024),
                Some(u32::MAX),
            ] {
                let base = SpeedLimits { download, upload }.fair();
                let game = base.during_game();
                assert!(game.download.unwrap() <= base.download.unwrap_or(u32::MAX));
                assert!(game.upload.unwrap() <= base.upload.unwrap_or(u32::MAX));
                assert!(game.download.unwrap() <= 512 * 1024);
                assert!(game.upload.unwrap() <= 128 * 1024);
                assert_eq!(game.fair(), game);
                assert_eq!(game.during_game(), game);
            }
        }
    }

    #[test]
    fn sharing_is_at_most_seven_times_slower() {
        let mib = 1024 * 1024;
        let limits = |download, upload| SpeedLimits { download, upload }.fair();
        // No download limit, no upload limit
        assert_eq!(limits(None, Some(mib)), limits(None, None));
        assert_eq!(limits(None, Some(mib)).upload, None);
        // Raised to a seventh of the download limit
        assert_eq!(limits(Some(7 * mib), Some(1)).upload, Some(mib));
        assert_eq!(limits(Some(7 * mib), Some(2 * mib)).upload, Some(2 * mib));
        // Sharing may be unlimited while downloading is limited
        assert_eq!(limits(Some(mib), None).upload, None);
        // Nothing below the minimum
        assert_eq!(limits(Some(1), Some(1)).download, Some(MIN_LIMIT_BPS));
        assert_eq!(limits(Some(1), Some(1)).upload, Some(MIN_LIMIT_BPS));
    }

    #[test]
    fn transfer_limits_are_enabled_by_default() {
        let limits = SpeedLimits::default();
        assert_eq!(limits.download, Some(20 * 1024 * 1024));
        assert_eq!(limits.upload, Some(10 * 1024 * 1024));
        assert_eq!(limits.fair(), limits);
    }

    #[test]
    fn paths() {
        assert_eq!(audio_path(7), "/api/v1/tracks/7/audio");
    }
}

/// Content-addressed attachment to the audio packets, independent of file tags.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetadataVariant {
    pub hash: String,
    pub kind: String,
    pub size: u32,
    pub selected: bool,
    /// Distinct signed respondents observed recently, plus this node if selected.
    pub peers: u32,
}
