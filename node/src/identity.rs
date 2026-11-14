// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! The node's signing key.
//!
//! Every track a node shares is announced in an entry signed with this key,
//! so that whoever returns the entry in a search, the original publisher
//! stays the same and cannot be forged. The public key is not secret: it is
//! shown next to every shared track.

use std::{
    fs,
    io::{self, Write},
    path::Path,
};

use ed25519_dalek::{Signer, SigningKey};

pub struct Identity {
    key: SigningKey,
}

impl Identity {
    /// Loads the key from `path`, creating it (owner-only) on first use.
    pub fn load_or_create(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let secret: [u8; 32] = bytes.try_into().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "the identity key is corrupt")
                })?;
                Ok(Self {
                    key: SigningKey::from_bytes(&secret),
                })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let mut secret = [0u8; 32];
                getrandom::fill(&mut secret).map_err(io::Error::other)?;
                let mut file = crate::fsx::private_file_options()
                    .create_new(true)
                    .open(path)?;
                file.write_all(&secret)?;
                file.sync_all()?;
                Ok(Self {
                    key: SigningKey::from_bytes(&secret),
                })
            }
            Err(e) => Err(e),
        }
    }

    pub fn generate() -> Self {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).expect("the operating system RNG is unavailable");
        Self {
            key: SigningKey::from_bytes(&secret),
        }
    }

    /// The public key, as lowercase hex.
    pub fn public_hex(&self) -> String {
        hex::encode(self.key.verifying_key().as_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message).to_bytes()
    }
}

/// A short, human comparable form of a public key.
pub fn fingerprint(public_hex: &str) -> &str {
    &public_hex[..public_hex.len().min(8)]
}
