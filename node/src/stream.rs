// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Plays tracks from other nodes without handing their bytes to the player.
//!
//! A file from someone else can't be validated before it has been
//! downloaded, and waiting for that would defeat streaming. So the player
//! never gets the file itself: the node demuxes and decodes it here with
//! Symphonia (pure Rust), using the reader for the one format the track
//! claims, and serves the result as plain 16-bit PCM in a WAV container
//! whose header the node writes itself. GStreamer only ever parses that.
//!
//! The WAV is served with `Range` support like a file. Its length is fixed
//! from the track's frame count, so a seek is just a request for a later
//! byte offset, which is mapped back to a frame and served after a
//! sample-accurate seek in the source. If the source ends early or turns
//! out to be damaged, the rest is padded with silence so that the length
//! the player was promised always holds.

use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::Mutex,
    time::Duration,
};

use anyhow::{bail, Context};
use hopea_common::AudioFormat;
use symphonia::core::{
    codecs::{
        audio::{AudioDecoder, AudioDecoderOptions},
        CodecParameters,
    },
    formats::{FormatReader, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream},
    units::{TimeBase, Timestamp},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt};

use crate::{config::Limits, ingest::open_reader};

pub const HEADER_LEN: u64 = 44;
const BYTES_PER_SAMPLE: u64 = 2;
/// Bytes handed to the HTTP body at a time
const CHUNK_BYTES: usize = 64 * 1024;

/// A decoder positioned at the start of a track.
pub struct PcmSource {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: TimeBase,
    pub sample_rate: u32,
    pub channels: u16,
    pub total_frames: u64,
}

impl PcmSource {
    /// Opens `source`, which claims to be `format` audio, reading only as
    /// much as is needed to know the stream's parameters.
    pub fn open(
        source: Box<dyn MediaSource>,
        format: AudioFormat,
        claimed_duration_ms: u64,
        limits: &Limits,
    ) -> anyhow::Result<Self> {
        let mss = MediaSourceStream::new(source, Default::default());
        let reader = open_reader(format, mss).map_err(|e| anyhow::anyhow!("{e}"))?;

        let track = reader
            .tracks()
            .iter()
            .find(|t| matches!(t.codec_params, Some(CodecParameters::Audio(_))))
            .context("no audio stream")?;
        let Some(CodecParameters::Audio(params)) = &track.codec_params else {
            unreachable!()
        };
        let sample_rate = params.sample_rate.context("unknown sample rate")?;
        if sample_rate == 0 || !limits.sample_rates.contains(&sample_rate) {
            bail!("unsupported sample rate {sample_rate}");
        }
        let channels = params.channels.as_ref().map_or(0, |c| c.count()) as u32;
        if channels == 0 || channels > limits.max_channels || channels > u32::from(u16::MAX) / 2 {
            bail!("unsupported channel count {channels}");
        }

        let max_frames = limits
            .max_duration
            .as_secs()
            .checked_mul(u64::from(sample_rate))
            .context("audio duration overflow")?;
        let claimed_frames = (u128::from(claimed_duration_ms) * u128::from(sample_rate) / 1000)
            .min(u128::from(max_frames)) as u64;
        let total_frames = track
            .num_frames
            .filter(|&n| n > 0)
            .unwrap_or(claimed_frames)
            .min(max_frames);
        if total_frames == 0 {
            bail!("the track has no length");
        }
        let data_len = total_frames
            .checked_mul(u64::from(channels))
            .and_then(|n| n.checked_mul(BYTES_PER_SAMPLE))
            .context("audio size overflow")?;
        if data_len > u64::from(u32::MAX) - HEADER_LEN {
            bail!("the track is too long to stream");
        }
        sample_rate
            .checked_mul(channels * 2)
            .context("audio byte rate overflow")?;

        let time_base = track
            .time_base
            .or_else(|| TimeBase::try_from_recip(sample_rate))
            .context("no time base")?;
        let track_id = track.id;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        Ok(Self {
            reader,
            decoder,
            track_id,
            time_base,
            sample_rate,
            channels: channels as u16,
            total_frames,
        })
    }

    fn block_align(&self) -> u64 {
        u64::from(self.channels) * BYTES_PER_SAMPLE
    }

    pub fn total_len(&self) -> u64 {
        HEADER_LEN + self.total_frames * self.block_align()
    }

    pub fn header(&self) -> [u8; HEADER_LEN as usize] {
        let data_len = (self.total_frames * self.block_align()) as u32;
        let byte_rate = self.sample_rate * self.block_align() as u32;
        let mut h = [0u8; HEADER_LEN as usize];
        h[0..4].copy_from_slice(b"RIFF");
        h[4..8].copy_from_slice(&(36 + data_len).to_le_bytes());
        h[8..16].copy_from_slice(b"WAVEfmt ");
        h[16..20].copy_from_slice(&16u32.to_le_bytes());
        h[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
        h[22..24].copy_from_slice(&self.channels.to_le_bytes());
        h[24..28].copy_from_slice(&self.sample_rate.to_le_bytes());
        h[28..32].copy_from_slice(&byte_rate.to_le_bytes());
        h[32..34].copy_from_slice(&(self.block_align() as u16).to_le_bytes());
        h[34..36].copy_from_slice(&16u16.to_le_bytes());
        h[36..40].copy_from_slice(b"data");
        h[40..44].copy_from_slice(&data_len.to_le_bytes());
        h
    }

    fn ts_to_frame(&self, ts: Timestamp) -> u64 {
        let v = i128::from(ts.get())
            * i128::from(self.time_base.numer.get())
            * i128::from(self.sample_rate)
            / i128::from(self.time_base.denom.get());
        v.clamp(0, i128::from(self.total_frames)) as u64
    }

    fn frame_to_ts(&self, frame: u64) -> Timestamp {
        let v = i128::from(frame) * i128::from(self.time_base.denom.get())
            / (i128::from(self.time_base.numer.get()) * i128::from(self.sample_rate));
        Timestamp::new(v.min(i128::from(i64::MAX)) as i64)
    }

    /// Produces bytes `start..end` of the WAV, passing them to `out` in
    /// chunks. Stops early, without error, when `out` returns false.
    /// Blocking: runs the decoder on the calling thread.
    pub fn produce(mut self, start: u64, end: u64, mut out: impl FnMut(Vec<u8>) -> bool) {
        let end = end.min(self.total_len());
        if start >= end {
            return;
        }
        let mut pos = start;
        if pos < HEADER_LEN {
            let header = self.header();
            let until = end.min(HEADER_LEN);
            if !out(header[pos as usize..until as usize].to_vec()) {
                return;
            }
            pos = until;
        }
        if pos >= end {
            return;
        }

        let block_align = self.block_align();
        let mut want = (pos - HEADER_LEN) / block_align;
        let mut writer = Writer {
            skip: ((pos - HEADER_LEN) % block_align) as usize,
            remaining: end - pos,
            buf: Vec::with_capacity(CHUNK_BYTES),
            out: &mut out,
            stopped: false,
        };

        if want > 0 {
            let to = SeekTo::Timestamp {
                ts: self.frame_to_ts(want),
                track_id: self.track_id,
            };
            if let Err(err) = self.reader.seek(SeekMode::Accurate, to) {
                // Decoding from the start and skipping is slow but correct
                tracing::debug!("Seek failed, decoding from the start: {err}");
            }
            self.decoder.reset();
        }

        let channels = usize::from(self.channels);
        let mut samples: Vec<i16> = Vec::new();
        let mut cursor: Option<u64> = None;
        let mut errors = 0u32;

        while writer.remaining > 0 && !writer.stopped && want < self.total_frames {
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => break,
                Err(err) => {
                    tracing::debug!("Source ended: {err}");
                    break;
                }
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let start_frame = *cursor.get_or_insert_with(|| self.ts_to_frame(packet.pts));
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    decoded.copy_to_vec_interleaved(&mut samples);
                }
                Err(err) => {
                    errors += 1;
                    tracing::debug!("Skipping a damaged packet: {err}");
                    if errors > 100 {
                        break;
                    }
                    continue;
                }
            }
            let frames = (samples.len() / channels) as u64;
            cursor = Some(start_frame + frames);
            if start_frame + frames <= want {
                continue;
            }
            if start_frame > want {
                // A gap in the source: keep time by filling it with silence
                let gap = (start_frame - want).min(self.total_frames - want);
                writer.silence(gap * block_align);
                want += gap;
            }
            if start_frame >= self.total_frames {
                break;
            }
            let from = (want - start_frame) as usize;
            let to = frames.min(self.total_frames - start_frame) as usize;
            if from < to {
                writer.samples(&samples[from * channels..to * channels]);
                want = start_frame + to as u64;
            }
        }

        // Keep the promised length even if the source fell short
        let remaining = writer.remaining;
        writer.silence(remaining);
        writer.flush();
    }
}

struct Writer<'a, F: FnMut(Vec<u8>) -> bool> {
    /// Bytes still to drop from the start of the first frame
    skip: usize,
    remaining: u64,
    buf: Vec<u8>,
    out: &'a mut F,
    stopped: bool,
}

impl<F: FnMut(Vec<u8>) -> bool> Writer<'_, F> {
    fn push(&mut self, mut bytes: &[u8]) {
        if self.skip > 0 {
            let n = self.skip.min(bytes.len());
            bytes = &bytes[n..];
            self.skip -= n;
        }
        while !bytes.is_empty() && self.remaining > 0 && !self.stopped {
            let n = bytes
                .len()
                .min(CHUNK_BYTES - self.buf.len())
                .min(self.remaining as usize);
            self.buf.extend_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
            self.remaining -= n as u64;
            if self.buf.len() == CHUNK_BYTES {
                self.flush();
            }
        }
    }

    fn samples(&mut self, samples: &[i16]) {
        let mut bytes = [0u8; 4096];
        for chunk in samples.chunks(bytes.len() / 2) {
            for (sample, out) in chunk.iter().zip(bytes.chunks_exact_mut(2)) {
                out.copy_from_slice(&sample.to_le_bytes());
            }
            self.push(&bytes[..chunk.len() * 2]);
            if self.stopped || self.remaining == 0 {
                break;
            }
        }
    }

    fn silence(&mut self, mut len: u64) {
        let zeros = [0u8; 4096];
        while len > 0 && self.remaining > 0 && !self.stopped {
            let n = len.min(zeros.len() as u64) as usize;
            self.push(&zeros[..n]);
            len -= n as u64;
        }
    }

    fn flush(&mut self) {
        if !self.buf.is_empty() && !self.stopped {
            let chunk = std::mem::replace(&mut self.buf, Vec::with_capacity(CHUNK_BYTES));
            self.stopped = !(self.out)(chunk);
        }
    }
}

/// A torrent file that may still be downloading, read synchronously for
/// Symphonia. Reads wait for the pieces they need, which the BitTorrent
/// session fetches first, and fail if nobody sends them in time.
pub struct TorrentFile<S> {
    stream: Mutex<S>,
    runtime: tokio::runtime::Handle,
    len: u64,
    timeout: Duration,
}

impl<S> TorrentFile<S> {
    pub fn new(stream: S, len: u64, runtime: tokio::runtime::Handle, timeout: Duration) -> Self {
        Self {
            len,
            stream: Mutex::new(stream),
            runtime,
            timeout,
        }
    }
}

fn timed_out() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "nobody sharing this track sent data in time",
    )
}

impl<S: AsyncRead + Unpin> Read for TorrentFile<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let stream = self.stream.get_mut().unwrap_or_else(|e| e.into_inner());
        self.runtime
            .block_on(tokio::time::timeout(self.timeout, stream.read(buf)))
            .map_err(|_| timed_out())?
    }
}

impl<S: AsyncSeek + Unpin> Seek for TorrentFile<S> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let stream = self.stream.get_mut().unwrap_or_else(|e| e.into_inner());
        self.runtime
            .block_on(tokio::time::timeout(self.timeout, stream.seek(pos)))
            .map_err(|_| timed_out())?
    }
}

impl<S: AsyncRead + AsyncSeek + Unpin + Send> MediaSource for TorrentFile<S> {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

/// Parses a `Range` header with a single byte range against a body of
/// `len` bytes. `Ok(None)` means the whole body; `Err` an unsatisfiable range.
pub fn parse_range(header: Option<&str>, len: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return Ok(None);
    };
    if spec.contains(',') {
        // Multiple ranges: serve the whole body, as allowed
        return Ok(None);
    }
    let (a, b) = spec.split_once('-').ok_or(())?;
    let (a, b) = (a.trim(), b.trim());
    let (start, end) = if a.is_empty() {
        // The last `b` bytes
        let n: u64 = b.parse().map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        (len.saturating_sub(n), len)
    } else {
        let start: u64 = a.parse().map_err(|_| ())?;
        let end = if b.is_empty() {
            len
        } else {
            b.parse::<u64>().map_err(|_| ())?.saturating_add(1).min(len)
        };
        (start, end)
    };
    if start >= len || start >= end {
        return Err(());
    }
    Ok(Some((start, end)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_output_is_chunked_even_for_a_large_decoded_packet() {
        let mut chunks = Vec::new();
        let mut out = |bytes| {
            chunks.push(bytes);
            true
        };
        let mut writer = Writer {
            skip: 0,
            remaining: (CHUNK_BYTES * 3) as u64,
            buf: Vec::new(),
            out: &mut out,
            stopped: false,
        };
        writer.samples(&vec![123i16; CHUNK_BYTES * 2]);
        writer.flush();
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|chunk| chunk.len() == CHUNK_BYTES));
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range(None, 100), Ok(None));
        assert_eq!(parse_range(Some("bytes=0-"), 100), Ok(Some((0, 100))));
        assert_eq!(parse_range(Some("bytes=10-19"), 100), Ok(Some((10, 20))));
        assert_eq!(parse_range(Some("bytes=90-200"), 100), Ok(Some((90, 100))));
        assert_eq!(parse_range(Some("bytes=-10"), 100), Ok(Some((90, 100))));
        assert_eq!(parse_range(Some("bytes=100-"), 100), Err(()));
        assert_eq!(parse_range(Some("bytes=5-1"), 100), Err(()));
        assert_eq!(parse_range(Some("bytes=a-b"), 100), Err(()));
        assert_eq!(parse_range(Some("bytes=0-1,5-6"), 100), Ok(None));
        assert_eq!(
            parse_range(Some("bytes=0-18446744073709551615"), 100),
            Ok(Some((0, 100)))
        );
        assert_eq!(
            parse_range(Some("bytes=18446744073709551615-"), 100),
            Err(())
        );
        assert_eq!(
            parse_range(Some("bytes=-18446744073709551615"), 100),
            Ok(Some((0, 100)))
        );
    }
}
