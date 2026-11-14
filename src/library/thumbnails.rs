// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Small pictures of cover art for the rows of search results.
//!
//! Results may number in the hundreds, so pictures are fetched only for rows
//! that are shown, a few at a time, scaled down as they are decoded, and kept
//! in a cache of bounded size.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
};

use gtk::{gdk, gio, prelude::*};

/// Pixels on a side, enough for a 40 px picture on a 2x screen
const SIZE: i32 = 96;
/// Pictures kept, about 37 KiB each
const MAX_CACHED: usize = 300;
/// Pictures fetched at once
const PARALLEL: usize = 4;

#[derive(Default)]
struct Cache {
    textures: HashMap<i64, gdk::Texture>,
    order: VecDeque<i64>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::default();
    static SLOTS: (async_channel::Sender<()>, async_channel::Receiver<()>) =
        async_channel::bounded(PARALLEL);
}

/// A track's picture, if it was fetched before.
pub fn cached(id: i64) -> Option<gdk::Texture> {
    CACHE.with(|c| c.borrow().textures.get(&id).cloned())
}

pub fn invalidate(id: i64) {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.textures.remove(&id);
        cache.order.retain(|key| *key != id);
    });
}

/// Fetches a track's picture from the node, scaled down.
pub async fn load(id: i64) -> Option<gdk::Texture> {
    if let Some(texture) = cached(id) {
        return Some(texture);
    }
    let (acquire, release) = SLOTS.with(|(tx, rx)| (tx.clone(), rx.clone()));
    acquire.send(()).await.ok()?;
    let texture = fetch(id).await;
    let _ = release.try_recv();

    let texture = texture?;
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.textures.insert(id, texture.clone()).is_none() {
            cache.order.push_back(id);
        }
        while cache.order.len() > MAX_CACHED {
            if let Some(old) = cache.order.pop_front() {
                cache.textures.remove(&old);
            }
        }
    });
    Some(texture)
}

async fn fetch(id: i64) -> Option<gdk::Texture> {
    // Fetched for a row that may be gone by now; another one may have it
    if let Some(texture) = cached(id) {
        return Some(texture);
    }
    let client = super::client()?;
    let bytes = client.cover(id).await.ok()?;
    // Decoded off the main thread; only the pixels come back
    let (pixels, width, height, stride, alpha) = gio::spawn_blocking(move || {
        let bytes = gtk::glib::Bytes::from_owned(hopea_node::ingest::reencode_cover(&bytes)?);
        let stream = gio::MemoryInputStream::from_bytes(&bytes);
        let pixbuf = gdk_pixbuf::Pixbuf::from_stream_at_scale(
            &stream,
            SIZE,
            SIZE,
            true,
            gio::Cancellable::NONE,
        )
        .ok()?;
        Some((
            pixbuf.read_pixel_bytes(),
            pixbuf.width(),
            pixbuf.height(),
            pixbuf.rowstride() as usize,
            pixbuf.has_alpha(),
        ))
    })
    .await
    .ok()??;
    let format = if alpha {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    Some(gdk::MemoryTexture::new(width, height, format, &pixels, stride).upcast())
}
