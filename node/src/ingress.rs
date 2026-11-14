// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Admission control for arbitrary clients of the public HTTP endpoint.
//! Never queue excess work, trust proxy headers, or allocate one rate bucket
//! per attacker-controlled node identity. IPv6 addresses share a /64 bucket.

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MAX_SOURCES: usize = 4096;
const SOURCE_TTL: Duration = Duration::from_secs(120);
const MAX_REQUESTS: usize = 48;
const MAX_SEARCHES: usize = 8;
const MAX_SEARCHES_PER_SOURCE: usize = 2;

struct Bucket {
    tokens: f64,
    updated: Instant,
}
impl Bucket {
    fn new(burst: f64, now: Instant) -> Self {
        Self {
            tokens: burst,
            updated: now,
        }
    }
    fn take(&mut self, now: Instant, rate: f64, burst: f64) -> bool {
        self.tokens =
            (self.tokens + now.duration_since(self.updated).as_secs_f64() * rate).min(burst);
        self.updated = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}
struct Source {
    requests: Bucket,
    searches: Bucket,
    searching: Arc<Semaphore>,
    seen: Instant,
}
struct Rates {
    requests: Bucket,
    searches: Bucket,
    sources: HashMap<IpAddr, Source>,
    swept: Instant,
}
pub(crate) struct Guard {
    rates: Mutex<Rates>,
    requests: Arc<Semaphore>,
    searches: Arc<Semaphore>,
}
struct Admission {
    _request: OwnedSemaphorePermit,
    _search: Option<OwnedSemaphorePermit>,
    _source_search: Option<OwnedSemaphorePermit>,
}

fn source_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6((u128::from(v6) & (u128::MAX << 64)).into()),
        },
        ip => ip,
    }
}

impl Guard {
    pub(crate) fn new() -> Arc<Self> {
        let now = Instant::now();
        Arc::new(Self {
            rates: Mutex::new(Rates {
                requests: Bucket::new(120.0, now),
                searches: Bucket::new(16.0, now),
                sources: HashMap::new(),
                swept: now,
            }),
            requests: Arc::new(Semaphore::new(MAX_REQUESTS)),
            searches: Arc::new(Semaphore::new(MAX_SEARCHES)),
        })
    }

    fn admit(&self, ip: IpAddr, search: bool, now: Instant) -> Option<Admission> {
        let request = self.requests.clone().try_acquire_owned().ok()?;
        let search_slot = if search {
            Some(self.searches.clone().try_acquire_owned().ok()?)
        } else {
            None
        };
        let mut rates = self.rates.lock().unwrap_or_else(|e| e.into_inner());
        if now.duration_since(rates.swept) >= Duration::from_secs(30) {
            rates
                .sources
                .retain(|_, source| now.duration_since(source.seen) < SOURCE_TTL);
            rates.swept = now;
        }
        let key = source_key(ip);
        if !rates.sources.contains_key(&key) && rates.sources.len() >= MAX_SOURCES {
            return None;
        }
        let source = rates.sources.entry(key).or_insert_with(|| Source {
            requests: Bucket::new(60.0, now),
            searches: Bucket::new(6.0, now),
            searching: Arc::new(Semaphore::new(MAX_SEARCHES_PER_SOURCE)),
            seen: now,
        });
        source.seen = now;
        let source_search = if search {
            Some(source.searching.clone().try_acquire_owned().ok()?)
        } else {
            None
        };
        if !source.requests.take(now, 20.0, 60.0)
            || (search && !source.searches.take(now, 1.0, 6.0))
        {
            return None;
        }
        if !rates.requests.take(now, 60.0, 120.0)
            || (search && !rates.searches.take(now, 4.0, 16.0))
        {
            return None;
        }
        Some(Admission {
            _request: request,
            _search: search_slot,
            _source_search: source_search,
        })
    }
}

pub(crate) async fn protect(
    State(guard): State<Arc<Guard>>,
    mut request: Request,
    next: Next,
) -> Response {
    // These are checked before query/body extraction and JSON deserialization.
    if request.uri().to_string().len() > 4096 {
        return StatusCode::URI_TOO_LONG.into_response();
    }
    let peer = request
        .extensions()
        .get::<ConnectInfo<PeerAddr>>()
        .map(|ConnectInfo(p)| p.0)
        .or_else(|| {
            request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(p)| *p)
        });
    let Some(peer) = peer else {
        return StatusCode::FORBIDDEN.into_response();
    };
    request.extensions_mut().insert(ConnectInfo(peer));
    let path = request.uri().path();
    let search = path == crate::p2p::SEARCH_PATH;
    let deadline = if path == crate::relay::UPLOAD_PATH || path == crate::relay::FETCH_PATH {
        Duration::from_secs(150)
    } else if path == crate::relay::POLL_PATH {
        Duration::from_secs(30)
    } else {
        Duration::from_secs(12)
    };
    let Some(_admission) = guard.admit(peer.ip(), search, Instant::now()) else {
        return (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "2")]).into_response();
    };
    match tokio::time::timeout(deadline, next.run(request)).await {
        Ok(response) => response,
        Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
    }
}

/// Bound sockets before HTTP headers exist, including slow/incomplete requests.
/// A finite connection lifetime also bounds peers that drip bytes forever.
pub(crate) struct Listener {
    inner: tokio::net::TcpListener,
    slots: Arc<Semaphore>,
    sources: Arc<Mutex<HashMap<IpAddr, usize>>>,
}
impl Listener {
    pub(crate) fn new(inner: tokio::net::TcpListener) -> Self {
        Self {
            inner,
            slots: Arc::new(Semaphore::new(128)),
            sources: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
pub(crate) struct Connection {
    inner: tokio::net::TcpStream,
    _slot: OwnedSemaphorePermit,
    source: IpAddr,
    sources: Arc<Mutex<HashMap<IpAddr, usize>>>,
    deadline: std::pin::Pin<Box<tokio::time::Sleep>>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        let mut sources = self.sources.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = sources.get_mut(&self.source) {
            *count -= 1;
            if *count == 0 {
                sources.remove(&self.source);
            }
        }
    }
}
impl axum::serve::Listener for Listener {
    type Io = Connection;
    type Addr = SocketAddr;
    async fn accept(&mut self) -> (Connection, SocketAddr) {
        loop {
            let slot = self
                .slots
                .clone()
                .acquire_owned()
                .await
                .expect("listener is open");
            let (inner, addr) = axum::serve::Listener::accept(&mut self.inner).await;
            let source = source_key(addr.ip());
            {
                let mut sources = self.sources.lock().unwrap_or_else(|e| e.into_inner());
                let count = sources.entry(source).or_default();
                if *count >= 16 {
                    continue;
                }
                *count += 1;
            }
            return (
                Connection {
                    inner,
                    _slot: slot,
                    source,
                    sources: self.sources.clone(),
                    deadline: Box::pin(tokio::time::sleep(Duration::from_secs(180))),
                },
                addr,
            );
        }
    }
    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.inner.local_addr()
    }
}
#[derive(Clone, Copy)]
pub(crate) struct PeerAddr(SocketAddr);
impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, Listener>>
    for PeerAddr
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, Listener>) -> Self {
        Self(*stream.remote_addr())
    }
}
impl tokio::io::AsyncRead for Connection {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::future::Future;
        if self.deadline.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(Err(std::io::ErrorKind::TimedOut.into()));
        }
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}
impl tokio::io::AsyncWrite for Connection {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        use std::future::Future;
        if self.deadline.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(Err(std::io::ErrorKind::TimedOut.into()));
        }
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn incomplete_connections_expire_and_release_their_slots() {
        use axum::serve::Listener as _;
        use tokio::io::AsyncReadExt;
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let mut listener = Listener::new(tcp);
        let _client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (mut connection, _) = listener.accept().await;
        assert_eq!(listener.slots.available_permits(), 127);
        connection.deadline = Box::pin(tokio::time::sleep(Duration::from_millis(10)));
        let mut byte = [0];
        assert_eq!(
            connection.read(&mut byte).await.unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
        drop(connection);
        assert_eq!(listener.slots.available_permits(), 128);
        assert!(listener.sources.lock().unwrap().is_empty());
    }
    #[test]
    fn flood_is_bounded_across_clients_and_recovers() {
        let guard = Guard::new();
        let now = Instant::now();
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..6 {
            assert!(guard.admit(a, true, now).is_some());
        }
        assert!(guard.admit(a, true, now).is_none());
        assert!(guard
            .admit("10.0.0.2".parse().unwrap(), true, now)
            .is_some());
        assert!(guard.admit(a, true, now + Duration::from_secs(2)).is_some());
        let guard = Guard::new();
        for i in 0..16 {
            assert!(guard
                .admit(IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 1, i)), true, now)
                .is_some());
        }
        assert!(guard.admit(a, true, now).is_none());
    }
    #[test]
    fn concurrent_searches_never_wait_and_ipv6_rotation_does_not_help() {
        let guard = Guard::new();
        let now = Instant::now();
        let a: IpAddr = "2001:db8::1".parse().unwrap();
        let b: IpAddr = "2001:db8::ffff".parse().unwrap();
        let one = guard.admit(a, true, now).unwrap();
        let two = guard.admit(b, true, now).unwrap();
        assert!(guard.admit(b, true, now).is_none());
        drop(one);
        drop(two);
        assert!(guard.admit(b, true, now).is_some());
        assert_eq!(
            source_key("::ffff:10.0.0.1".parse().unwrap()),
            source_key("10.0.0.1".parse().unwrap())
        );
    }
    #[test]
    fn source_table_is_bounded_without_evicting_active_limits() {
        let guard = Guard::new();
        let now = Instant::now();
        // Artificially fill the table without waiting for rate refills.
        let mut rates = guard.rates.lock().unwrap();
        for i in 0..MAX_SOURCES {
            rates.sources.insert(
                IpAddr::V4((i as u32).into()),
                Source {
                    requests: Bucket::new(0.0, now),
                    searches: Bucket::new(0.0, now),
                    searching: Arc::new(Semaphore::new(2)),
                    seen: now,
                },
            );
        }
        drop(rates);
        let ip = "192.0.2.1".parse().unwrap();
        assert!(guard.admit(ip, true, now).is_none());
        assert!(guard.admit(ip, true, now + SOURCE_TTL).is_some());
    }
}
