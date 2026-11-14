// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! A node without the player, for a computer that is always on: it keeps
//! the tracks shared from it available while everyone else is offline.

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::Context;
use clap::{Parser, Subcommand};
use hopea_common::api::{SpeedLimits, MAX_SHARING_SLOWDOWN};
use hopea_node::{
    auth::load_or_create_token,
    config::{Config, DEFAULT_P2P_PORT},
    entry::MAX_NAME_CHARS,
    ingest, Node,
};
use tracing_subscriber::EnvFilter;

const MIB: u64 = 1024 * 1024;

#[derive(Parser)]
#[command(version, about = "Peer-to-peer music sharing node for Hopea")]
struct Cli {
    /// Directory holding the catalog, the key and the music
    #[arg(long, env = "HOPEA_DATA_DIR", global = true)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the node
    Serve {
        /// Address of the API the player connects to
        #[arg(long, env = "HOPEA_LISTEN", default_value = "127.0.0.1:8740")]
        listen: SocketAddr,

        /// Address other nodes search this one on
        #[arg(long, default_value_t = SocketAddr::from(([0u16; 8], DEFAULT_P2P_PORT)))]
        p2p_listen: SocketAddr,

        /// BitTorrent port (0 picks one)
        #[arg(long, default_value_t = 0)]
        bt_port: u16,

        /// Name shown next to the tracks shared from here
        #[arg(long)]
        name: Option<String>,

        /// Another node to always search (host:port), may be repeated
        #[arg(long = "peer")]
        peers: Vec<SocketAddr>,

        /// Space for tracks shared from here, in GiB
        #[arg(long, default_value_t = 20)]
        quota_gib: u64,

        /// Space for tracks downloaded from others, in GiB
        #[arg(long, default_value_t = 5)]
        cache_gib: u64,

        /// Largest accepted file, in MiB
        #[arg(long, default_value_t = 512)]
        max_upload_mib: u64,

        /// Don't use the public DHT; only search the --peer nodes
        #[arg(long)]
        no_dht: bool,

        /// Don't ask the router to forward ports
        #[arg(long)]
        no_upnp: bool,

        /// Don't relay for nodes that others can't reach
        #[arg(long)]
        no_relay: bool,

        /// A node to try first as a relay (host:port), may be repeated
        #[arg(long = "relay")]
        relays: Vec<SocketAddr>,

        /// Always go through a relay, even if this node seems reachable
        #[arg(long)]
        force_relay: bool,

        /// Download speed limit, in KiB/s
        #[arg(long)]
        download_limit_kib: Option<u32>,

        /// Sharing speed limit, in KiB/s. Needs a download limit, and is
        /// raised to at least a seventh of it
        #[arg(long)]
        upload_limit_kib: Option<u32>,
    },
    /// Print the token the player needs to connect, creating it if needed
    Token {
        /// Replace the token with a new one
        #[arg(long)]
        rotate: bool,
    },
    /// Run the upload validation on a local file and print the result
    Check { file: PathBuf },
}

fn default_data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    let legacy = base.join("anysik-node");
    if legacy.is_dir() {
        legacy
    } else {
        base.join("hopea-node")
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("hopea_node=info,librqbit=warn")),
        )
        .init();

    let cli = Cli::parse();
    let data_dir = cli.data_dir.unwrap_or_else(default_data_dir);
    match cli.command {
        Command::Serve {
            listen,
            p2p_listen,
            bt_port,
            name,
            peers,
            quota_gib,
            cache_gib,
            max_upload_mib,
            no_dht,
            no_upnp,
            no_relay,
            relays,
            force_relay,
            download_limit_kib,
            upload_limit_kib,
        } => {
            if !listen.ip().is_loopback() {
                anyhow::bail!(
                    "refusing to put the player API on {listen}: it is meant for this computer \
                     only; use an SSH tunnel or a VPN to reach it from elsewhere"
                );
            }
            if let Some(ref name) = name {
                if name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
                    anyhow::bail!("names are at most {MAX_NAME_CHARS} characters");
                }
            }
            std::fs::create_dir_all(&data_dir)?;
            let token = load_or_create_token(&data_dir.join("api-token"), false)
                .context("unable to set up the API token")?;

            let mut config = Config::new(data_dir);
            config.listen = listen;
            config.p2p_listen = p2p_listen;
            config.bt_port = bt_port;
            config.name = name;
            config.peers = peers;
            config.quota_bytes = quota_gib * 1024 * MIB;
            config.cache_bytes = cache_gib * 1024 * MIB;
            config.limits.max_upload_bytes = max_upload_mib * MIB;
            config.dht = !no_dht;
            config.upnp = !no_upnp;
            config.relay = !no_relay;
            config.relays = relays;
            config.force_relay = force_relay;
            config.speed = SpeedLimits {
                download: download_limit_kib
                    .or(Some(20480))
                    .filter(|&k| k > 0)
                    .map(|k| k.saturating_mul(1024)),
                upload: upload_limit_kib
                    .or(Some(10240))
                    .filter(|&k| k > 0)
                    .map(|k| k.saturating_mul(1024)),
            };
            let fair = config.speed.fair();
            if fair != config.speed {
                println!(
                    "Sharing may be at most {MAX_SHARING_SLOWDOWN} times slower than downloading; \
                     limits in effect: download {:?} B/s, sharing {:?} B/s",
                    fair.download, fair.upload
                );
            }
            if no_dht {
                config.search_timeout = Duration::from_secs(5);
            }

            let node = Node::start(config, &token).await?;
            println!("Player API: http://{}", node.api_addr);
            println!("Token: run `hopea-node token`");
            let _ = tokio::signal::ctrl_c().await;
            node.shutdown().await;
        }
        Command::Token { rotate } => {
            std::fs::create_dir_all(&data_dir)?;
            println!(
                "{}",
                load_or_create_token(&data_dir.join("api-token"), rotate)?
            );
        }
        Command::Check { file } => {
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .context("the file name is not valid UTF-8")?
                .to_string();
            let work = tempfile::tempdir()?;
            let limits = Config::new(PathBuf::new()).limits;
            match tokio::task::spawn_blocking(move || {
                ingest::ingest(&file, &name, work.path(), &limits)
            })
            .await?
            {
                Ok(i) => {
                    println!("Accepted: {:?} / {}", i.format, i.codec);
                    println!(
                        "  {} Hz, {} channel(s), {} ms",
                        i.sample_rate, i.channels, i.duration_ms
                    );
                    println!("  {:?}", i.tags);
                    println!(
                        "  cover art: {}",
                        if i.cover_jpeg.is_some() { "yes" } else { "no" }
                    );
                    println!(
                        "  {} bytes stored, {} not audio (allowed {})",
                        i.size, i.overhead_bytes, i.overhead_allowed
                    );
                    println!("  sha256 {}", i.sha256);
                }
                Err(rejection) => {
                    println!("Rejected ({}): {rejection}", rejection.code());
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}
