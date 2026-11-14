// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! One bounded grammar for network validation and playback. Invalid timestamp
//! syntax is an error, never silently treated as an unrelated metadata tag.
use std::fmt;
pub const MAX_BYTES: usize = 256 * 1024;
pub const MAX_LINES: usize = 2000;
pub const MAX_LINE_BYTES: usize = 16 * 1024;
pub const MAX_TIMESTAMP_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub time_ms: Option<u64>,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Lyrics {
    pub lines: Vec<Line>,
}
impl Lyrics {
    pub fn synced(&self) -> bool {
        self.lines.first().is_some_and(|l| l.time_ms.is_some())
    }
    pub fn line_at(&self, position_ms: u64) -> Option<usize> {
        if !self.synced() {
            return None;
        }
        self.lines
            .partition_point(|l| l.time_ms.is_some_and(|t| t <= position_ms))
            .checked_sub(1)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub reason: &'static str,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Lyrics line {}: {}", self.line, self.reason)
    }
}
impl std::error::Error for Error {}

fn number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.bytes().try_fold(0u64, |n, b| {
        n.checked_mul(10)?.checked_add(u64::from(b - b'0'))
    })
}
/// No signed numbers, non-finite values, silently truncated precision or overflow.
pub fn timestamp(tag: &str) -> Option<u64> {
    let (minutes, seconds) = tag.split_once(':')?;
    let (seconds, fraction) = match seconds.split_once(['.', ':']) {
        Some((s, f)) if (1..=3).contains(&f.len()) => (s, Some(f)),
        Some(_) => return None,
        None => (seconds, None),
    };
    if seconds.len() > 2 {
        return None;
    }
    let sec = number(seconds)?;
    if sec >= 60 {
        return None;
    }
    let millis = match fraction {
        None => 0,
        Some(f) => number(f)?.checked_mul(10u64.pow(3 - f.len() as u32))?,
    };
    let time = number(minutes)?
        .checked_mul(60_000)?
        .checked_add(sec * 1000)?
        .checked_add(millis)?;
    (time <= MAX_TIMESTAMP_MS).then_some(time)
}
fn clock_like(s: &str) -> bool {
    s.starts_with(['+', '-']) || s.chars().next().is_some_and(|c| c.is_numeric()) || s.contains(':')
}

pub fn parse(text: &str, duration_ms: Option<u64>) -> Result<Lyrics, Error> {
    let error = |line, reason| Error { line, reason };
    if text.len() > MAX_BYTES {
        return Err(error(1, "document exceeds 256 KiB"));
    }
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let limit = duration_ms
        .unwrap_or(MAX_TIMESTAMP_MS)
        .min(MAX_TIMESTAMP_MS);
    let mut timed = Vec::new();
    let mut plain = Vec::new();
    let mut offset = false;
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        if line > MAX_LINES {
            return Err(error(line, "too many lines"));
        }
        if raw.len() > MAX_LINE_BYTES {
            return Err(error(line, "line is too long"));
        }
        if raw.chars().any(|c| {
            (c.is_control() && c != '\t')
                || matches!(c,'\u{2028}'..='\u{202E}'|'\u{2066}'..='\u{2069}'|'\u{FEFF}')
        }) {
            return Err(error(line, "control or direction override character"));
        }
        let mut rest = raw.trim();
        let mut times = Vec::new();
        let mut metadata = false;
        while let Some(inside) = rest.strip_prefix('[') {
            let Some(end) = inside.find(']') else {
                if clock_like(inside) {
                    return Err(error(line, "unterminated timestamp"));
                }
                break;
            };
            let tag = inside[..end].trim();
            let name = tag.split_once(':').map(|(n, _)| n.to_ascii_lowercase());
            match name.as_deref() {
                Some("offset") => {
                    // Nonzero offsets were never implemented by the player. Reject
                    // instead of validating one timeline and playing another.
                    if offset || tag.split_once(':').unwrap().1.trim() != "0" {
                        return Err(error(
                            line,
                            "only one zero offset is supported; use explicit timestamps",
                        ));
                    }
                    offset = true;
                    metadata = true;
                }
                Some("length") => {
                    let t = timestamp(tag.split_once(':').unwrap().1.trim())
                        .ok_or_else(|| error(line, "invalid length tag"))?;
                    if t > limit {
                        return Err(error(line, "length exceeds the song duration"));
                    }
                    metadata = true;
                }
                Some("ar" | "ti" | "al" | "au" | "by" | "re" | "ve") => metadata = true,
                _ if clock_like(tag) => {
                    let t = timestamp(tag)
                        .ok_or_else(|| error(line, "invalid or negative timestamp"))?;
                    if t > limit {
                        return Err(error(line, "timestamp exceeds the song duration"));
                    }
                    if timed.len() + plain.len() + times.len() >= MAX_LINES {
                        return Err(error(line, "too many expanded timestamp lines"));
                    }
                    times.push(t);
                }
                _ => break,
            }
            rest = inside[end + 1..].trim_start();
        }
        // Enhanced word-level LRC is not supported. Never accept hidden timing
        // directives as though they were ordinary text.
        for segment in rest.split('<').skip(1) {
            let tag = segment.split_once('>').map_or(segment, |(tag, _)| tag);
            if tag.contains(':') && clock_like(tag) {
                return Err(error(line, "word timestamps are not supported"));
            }
        }
        if metadata && times.is_empty() {
            if !rest.is_empty() {
                return Err(error(line, "text after a metadata tag"));
            }
            continue;
        }
        if timed.len() + plain.len() >= MAX_LINES {
            return Err(error(line, "too many lines"));
        }
        let text = rest.chars().take(300).collect::<String>();
        if times.is_empty() {
            plain.push(Line {
                time_ms: None,
                text,
            });
        } else {
            timed.extend(times.into_iter().map(|t| Line {
                time_ms: Some(t),
                text: text.clone(),
            }));
        }
    }
    let lines = if timed.is_empty() {
        while plain.last().is_some_and(|l| l.text.is_empty()) {
            plain.pop();
        }
        let first = plain
            .iter()
            .position(|l| !l.text.is_empty())
            .unwrap_or(plain.len());
        plain.split_off(first)
    } else {
        timed.sort_by_key(|l| l.time_ms);
        timed
    };
    Ok(Lyrics { lines })
}
pub fn validate(text: &str, duration_ms: Option<u64>) -> Result<(), Error> {
    parse(text, duration_ms).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hostile_times_never_become_plain_text_or_metadata() {
        for t in [
            "-00:01",
            "00:-01",
            "+01:00",
            "00:60",
            "00:NaN",
            "NaN:00",
            "00:inf",
            "00:01.1234",
            "00:01.",
            "18446744073709551615:59.999",
            "307445734561826:00",
            "１:00",
            "00:01.１２３",
        ] {
            assert!(
                validate(&format!("[00:00]Good\n[{t}]Bad"), Some(1000)).is_err(),
                "{t}"
            );
        }
        for text in [
            "[00:01",
            "[offset:-1000]\n[00:00]Bad",
            "[offset:9223372036854775807]\n[00:00]Bad",
            "[00:00]bad<00:-01>word",
            "[length:-1:00]\ntext",
        ] {
            assert!(validate(text, Some(1000)).is_err(), "{text}");
        }
        assert!(validate("[00:01.001]Too late", Some(1000)).is_err());
        assert!(validate("[00:01.000]End", Some(1000)).is_ok());
    }
    #[test]
    fn valid_lrc_is_sorted_and_bounded() {
        let l = parse(
            "[ar:Artist]\n[offset:0]\n[00:01.1][00:03.001]Chorus\n[00:02]Other",
            Some(4000),
        )
        .unwrap();
        assert_eq!(
            l.lines
                .iter()
                .map(|l| l.time_ms.unwrap())
                .collect::<Vec<_>>(),
            [1100, 2000, 3001]
        );
        assert_eq!(l.line_at(2001), Some(1));
        assert_eq!(l.line_at(0), None);
        assert!(validate(
            &format!("{}text", "[00:00]".repeat(MAX_LINES + 1)),
            Some(1000)
        )
        .is_err());
        assert!(validate(&"\n".repeat(MAX_LINES + 1), None).is_err());
        assert!(validate(&"x".repeat(MAX_LINE_BYTES + 1), None).is_err());
        assert!(validate(&"x".repeat(MAX_BYTES + 1), None).is_err());
        for c in ['\0', '\u{1b}', '\u{202e}', '\u{2066}'] {
            assert!(validate(&format!("text{c}"), None).is_err());
        }
        assert!(validate("", Some(1000)).is_ok());
        assert_eq!(
            parse("<script>plain text</script>", None).unwrap().lines[0].text,
            "<script>plain text</script>"
        );
    }
}
