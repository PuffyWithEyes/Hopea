// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

// The peer-to-peer node (see `node/` in the source tree) runs inside the
// player, on a thread of its own with a Tokio runtime, since it is written
// against Tokio and the player against the GLib main loop. The two talk
// over the node's HTTP API on loopback, exactly as they would if the node
// were a separate program: the player gets the address and a fresh token
// for every start, and nothing about the token is stored.

use std::{net::SocketAddr, path::PathBuf, sync::Mutex, thread};

use hopea_common::api::SpeedLimits;
use hopea_node::{auth::generate_token, config::Config, Node};
use log::{debug, warn};

/// Asks the running node to stop, and waits for it.
struct Running {
    ipv4_only: bool,
    gaming: tokio::sync::watch::Sender<bool>,
    stop: tokio::sync::oneshot::Sender<()>,
    thread: thread::JoinHandle<()>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

static CATALOG_CHANGES: std::sync::LazyLock<tokio::sync::watch::Sender<u64>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::Sender::new(0));
static REACHABILITY_CHANGES: std::sync::LazyLock<tokio::sync::watch::Sender<u64>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::Sender::new(0));

pub(crate) fn catalog_changes() -> tokio::sync::watch::Receiver<u64> {
    CATALOG_CHANGES.subscribe()
}

pub(crate) fn reachability_changes() -> tokio::sync::watch::Receiver<u64> {
    REACHABILITY_CHANGES.subscribe()
}

pub(crate) fn notify_catalog_changed() {
    CATALOG_CHANGES.send_modify(|revision| *revision = revision.wrapping_add(1));
}

/// Where the node keeps its catalog, key and music.
fn data_dir() -> PathBuf {
    crate::paths::data_dir().join("library")
}

/// Starts the node, unless it is already running. Resolves to the address
/// of its API and the token for it.
pub async fn start(
    name: Option<String>,
    peers: Vec<SocketAddr>,
    speed: SpeedLimits,
    ipv4_only: bool,
) -> Result<(String, String), String> {
    let (ready_tx, ready_rx) = async_channel::bounded(1);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let (gaming_tx, mut gaming_rx) = tokio::sync::watch::channel(crate::gaming::active());

    {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if running.is_some() {
            return Err("The node is already running".into());
        }

        let token = generate_token();
        let mut config = Config::new(data_dir());
        config.name = name;
        config.peers = peers;
        config.speed = speed;
        config.ipv4_only = ipv4_only;

        let thread = thread::Builder::new()
            .name("hopea-node".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("hopea-node-worker")
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = ready_tx.send_blocking(Err(err.to_string()));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let node = match Node::start(config, &token).await {
                        Ok(node) => node,
                        Err(err) => {
                            let _ = ready_tx.send(Err(format!("{err:#}"))).await;
                            return;
                        }
                    };
                    let url = format!("http://{}", node.api_addr);
                    let mut changes = node.state.changes.subscribe();
                    let mut reachability = node.state.reach.subscribe();
                    node.state
                        .torrents
                        .set_gaming(*gaming_rx.borrow_and_update());
                    let _ = ready_tx.send(Ok((url, token))).await;
                    tokio::pin!(stop_rx);
                    loop {
                        tokio::select! {
                            _ = &mut stop_rx => break,
                            changed = changes.changed() => {
                                if changed.is_err() { break; }
                                notify_catalog_changed();
                            }
                            changed = reachability.changed() => {
                                if changed.is_err() { break; }
                                REACHABILITY_CHANGES.send_modify(|revision| *revision = revision.wrapping_add(1));
                            }
                            changed = gaming_rx.changed() => {
                                if changed.is_err() { break; }
                                node.state.torrents.set_gaming(*gaming_rx.borrow_and_update());
                            }
                        }
                    }
                    debug!("Stopping the library node");
                    node.shutdown().await;
                });
            })
            .map_err(|e| e.to_string())?;

        *running = Some(Running {
            ipv4_only,
            gaming: gaming_tx,
            stop: stop_tx,
            thread,
        });
    }

    let result = ready_rx
        .recv()
        .await
        .unwrap_or_else(|_| Err("The node stopped unexpectedly".into()));
    if result.is_err() {
        stop();
    }
    result
}

pub fn set_gaming(active: bool) {
    if let Some(node) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        node.gaming.send_replace(active);
    }
}

pub fn ipv4_only() -> bool {
    RUNNING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|node| node.ipv4_only)
}

/// Stops the node, if it is running, and waits for it to finish so that
/// its files are left in order.
pub fn stop() {
    let running = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(running) = running {
        let _ = running.stop.send(());
        if running.thread.join().is_err() {
            warn!("The library node panicked");
        }
    }
}
