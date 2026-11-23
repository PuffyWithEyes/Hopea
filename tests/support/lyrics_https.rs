// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
// Run with tests/test_lyrics_https.sh: actual loopback TLS, without GTK or a
// request to a public service. Test certificates exist only in a temporary dir.

use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::Arc,
    thread::{self, JoinHandle},
    time::Instant,
};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(PathBuf::from(std::env::var_os("HOPEA_TLS_FIXTURES").unwrap()).join(name))
        .unwrap()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn trusted_client() -> reqwest::Client {
    client_builder("Hopea-test")
        .unwrap()
        .tls_certs_only([reqwest::Certificate::from_der(&fixture("ca.der")).unwrap()])
        .no_proxy()
        .build()
        .unwrap()
}

fn serve(response: &[u8]) -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![rustls::pki_types::CertificateDer::from(fixture(
            "server.der",
        ))],
        rustls::pki_types::PrivatePkcs8KeyDer::from(fixture("server-key.der")).into(),
    )
    .unwrap();
    let response = response.to_vec();
    let worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "TLS client never connected");
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        let mut stream = rustls::StreamOwned::new(connection, socket);
        let mut request = Vec::new();
        let mut chunk = [0; 1024];
        while !request.windows(4).any(|tail| tail == b"\r\n\r\n") {
            match stream.read(&mut chunk) {
                // Certificate rejection ends the handshake before HTTP.
                Ok(0) | Err(_) => return,
                Ok(n) => request.extend_from_slice(&chunk[..n]),
            }
            assert!(request.len() < 8192);
        }
        let _ = stream.write_all(&response);
        let _ = stream.flush();
    });
    (port, worker)
}

#[test]
fn retrieves_a_verified_https_reply_without_gio() {
    let (port, worker) =
        serve(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]");
    let answer = runtime().block_on(fetch_with_client(
        &trusted_client(),
        &format!("https://localhost:{port}/api/search"),
        1024,
    ));
    assert_eq!(answer.unwrap(), Some(b"[]".to_vec()));
    worker.join().unwrap();
}

#[test]
fn rejects_untrusted_certificates_and_wrong_hostnames() {
    let (port, worker) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]");
    let untrusted = client_builder("Hopea-test")
        .unwrap()
        .no_proxy()
        .build()
        .unwrap();
    let result = runtime().block_on(fetch_with_client(
        &untrusted,
        &format!("https://localhost:{port}/"),
        1024,
    ));
    assert!(matches!(result, Err(FetchError::Transport(_))));
    worker.join().unwrap();

    let (port, worker) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]");
    // The trusted test certificate is valid only for localhost, not this IP.
    let result = runtime().block_on(fetch_with_client(
        &trusted_client(),
        &format!("https://127.0.0.1:{port}/"),
        1024,
    ));
    assert!(matches!(result, Err(FetchError::Transport(_))));
    worker.join().unwrap();
}

#[test]
fn rejects_http_and_https_to_http_redirects() {
    assert!(matches!(
        runtime().block_on(fetch("http://127.0.0.1:1/", "Hopea-test", 1024)),
        Err(FetchError::Transport(_))
    ));
    let (port, worker) = serve(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let result = runtime().block_on(fetch_with_client(
        &trusted_client(),
        &format!("https://localhost:{port}/"),
        1024,
    ));
    assert!(matches!(result, Err(FetchError::Transport(_))));
    worker.join().unwrap();
}

#[test]
fn handles_not_found_and_rejects_service_errors() {
    for (response, not_found) in [
        (
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".as_slice(),
            true,
        ),
        (
            b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .as_slice(),
            false,
        ),
    ] {
        let (port, worker) = serve(response);
        let result = runtime().block_on(fetch_with_client(
            &trusted_client(),
            &format!("https://localhost:{port}/"),
            1024,
        ));
        if not_found {
            assert_eq!(result.unwrap(), None);
        } else {
            assert!(matches!(result, Err(FetchError::UnexpectedStatus)));
        }
        worker.join().unwrap();
    }
}

#[test]
fn limits_content_length_and_actual_chunked_bytes() {
    for response in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\n12345\r\n5\r\n67890\r\n0\r\n\r\n".as_slice(),
    ] {
        let (port, worker) = serve(response);
        let result = runtime().block_on(fetch_with_client(
            &trusted_client(),
            &format!("https://localhost:{port}/"),
            8,
        ));
        assert!(matches!(result, Err(FetchError::TooLarge)));
        worker.join().unwrap();
    }
}
