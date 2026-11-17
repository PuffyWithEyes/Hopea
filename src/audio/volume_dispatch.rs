// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keep Android's synchronous audio-driver volume calls off the GTK thread.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc::{sync_channel, SyncSender},
    Arc,
};

#[derive(Debug)]
pub(super) struct VolumeDispatch {
    latest: Arc<AtomicU64>,
    wake: SyncSender<()>,
}

impl VolumeDispatch {
    pub(super) fn new(mut apply: impl FnMut(f64) + Send + 'static) -> Self {
        let latest = Arc::new(AtomicU64::new(1.0_f64.to_bits()));
        let value = latest.clone();
        let (wake, requests) = sync_channel(1);
        std::thread::Builder::new()
            .name("hopea-volume".into())
            .spawn(move || {
                while requests.recv().is_ok() {
                    apply(f64::from_bits(value.load(Ordering::Acquire)));
                }
            })
            .expect("Unable to start audio volume worker");
        Self { latest, wake }
    }

    pub(super) fn set(&self, volume: f64) {
        if !volume.is_finite() {
            return;
        }
        self.latest
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Release);
        // At most one pending wake-up. A stalled driver cannot block GTK or
        // accumulate one task/thread per slider motion; the latest value wins.
        let _ = self.wake.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc::channel, time::Duration};

    #[test]
    fn blocked_driver_does_not_block_slider_and_coalesces_changes() {
        let (applied, values) = channel();
        let (resume, pause) = channel();
        let volume = VolumeDispatch::new(move |value| {
            applied.send(value).unwrap();
            pause.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        volume.set(0.5);
        assert_eq!(values.recv_timeout(Duration::from_secs(2)).unwrap(), 0.5);
        // The worker remains blocked until every simulated slider update ends.
        for value in 0..10_000 {
            volume.set(value as f64 / 10_000.0);
        }
        volume.set(0.25);
        volume.set(f64::NAN);
        volume.set(f64::INFINITY);
        resume.send(()).unwrap();
        assert_eq!(values.recv_timeout(Duration::from_secs(2)).unwrap(), 0.25);
        drop(volume);
        resume.send(()).unwrap();
        assert!(values.recv_timeout(Duration::from_secs(2)).is_err());
    }
}
