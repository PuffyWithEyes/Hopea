// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Types shared between the Hopea client and the shared library server.
//!
//! The server is the only authority on whether an upload is acceptable; the
//! client uses [`AudioFormat::sniff`] purely to give early feedback before
//! sending a file over the network.

pub mod api;
mod format;
pub mod lyrics;
pub mod service_import;

pub use format::{describe_non_audio, AudioFormat, SNIFF_LEN};
