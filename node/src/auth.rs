// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bearer token authentication for the player's API.
//!
//! The API listens on loopback, but other programs on the same computer
//! (and web pages, through the browser) can reach loopback too, so every
//! request needs the token. The player built into the same process gets a
//! fresh token on every start; a headless node keeps one in its data
//! directory, readable by its owner only.
//!
//! Tokens are 256 bits from the operating system's CSPRNG. Only a SHA-256
//! hash is kept in memory for comparison.

use std::{
    fs,
    io::{self, Write},
    path::Path,
};

use axum::{extract::FromRequestParts, http::request::Parts};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};

use crate::{api::ApiFailure, AppState};

const TOKEN_PREFIX: &str = "anysik_";
const MAX_TOKEN_LEN: usize = 128;

pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system RNG is unavailable");
    format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

pub fn hash_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Reads the token stored at `path`, creating one if there is none.
pub fn load_or_create_token(path: &Path, rotate: bool) -> io::Result<String> {
    if !rotate {
        match fs::read_to_string(path) {
            Ok(token) if token.trim().starts_with(TOKEN_PREFIX) => {
                return Ok(token.trim().to_string())
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    let token = generate_token();
    let tmp = path.with_extension("tmp");
    let mut file = crate::fsx::private_file_options()
        .create(true)
        .truncate(true)
        .open(&tmp)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(token)
}

/// Compares in constant time, so that response timing says nothing about
/// how much of a guess was right.
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Proof that a request carried the API token.
pub struct Authorized;

impl FromRequestParts<AppState> for Authorized {
    type Rejection = ApiFailure;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiFailure> {
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|t| t.starts_with(TOKEN_PREFIX) && t.len() <= MAX_TOKEN_LEN)
            .ok_or_else(ApiFailure::unauthorized)?;
        if same(&hash_token(token), &state.token_hash) {
            Ok(Authorized)
        } else {
            Err(ApiFailure::unauthorized())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_and_prefixed() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert!(a.starts_with(TOKEN_PREFIX));
        assert_eq!(a.len(), TOKEN_PREFIX.len() + 43);
        assert!(same(&hash_token(&a), &hash_token(&a)));
        assert!(!same(&hash_token(&a), &hash_token(&b)));
    }

    #[test]
    fn stored_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        let a = load_or_create_token(&path, false).unwrap();
        assert_eq!(load_or_create_token(&path, false).unwrap(), a);
        let b = load_or_create_token(&path, true).unwrap();
        assert_ne!(a, b);
    }
}
