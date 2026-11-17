// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hostile clients use raw HTTP, without relying on Hopea's client checks.
use hopea_node::{auth::generate_token, config::Config, Node};
use reqwest::StatusCode;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn search_flood_ignores_forged_identity_headers_and_preserves_local_api() {
    let dir = tempfile::tempdir().unwrap();
    let token = generate_token();
    let node = Node::start(Config::isolated(dir.path().join("node")), &token)
        .await
        .unwrap();
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let mut rejected = 0;
    for i in 0..30 {
        let reply = http
            .get(format!(
                "http://{}/p2p/v1/search?q=arbitrary-{i}",
                node.p2p_addr
            ))
            .header("X-Forwarded-For", format!("192.0.2.{i}"))
            .header("Forwarded", format!("for=192.0.2.{i}"))
            .send()
            .await
            .unwrap();
        if reply.status() == StatusCode::TOO_MANY_REQUESTS {
            rejected += 1;
            assert!(reply.headers().contains_key("retry-after"));
        } else {
            assert_eq!(reply.status(), StatusCode::OK);
        }
    }
    assert!(rejected >= 20, "flood was not limited: {rejected} refusals");
    let reply = http
        .get(format!("http://{}/api/v1/me", node.api_addr))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        http.get(format!(
            "http://{}/p2p/v1/search?q=recovered",
            node.p2p_addr
        ))
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn oversized_public_bodies_and_queries_are_rejected_before_parsing() {
    let dir = tempfile::tempdir().unwrap();
    let node = Node::start(Config::isolated(dir.path().join("node")), &generate_token())
        .await
        .unwrap();
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let reply = http
        .post(format!("http://{}/p2p/v1/relay/register", node.p2p_addr))
        .header("Content-Type", "application/json")
        .body(vec![b' '; 65537])
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let reply = http
        .get(format!(
            "http://{}/p2p/v1/search?q={}",
            node.p2p_addr,
            "x".repeat(5000)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::URI_TOO_LONG);
}

#[tokio::test]
async fn chunked_peer_response_cannot_bypass_json_size_limit() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        socket.read(&mut request).await.unwrap();
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        for chunk in [b"\"1234567".as_slice(), b"89012345\"".as_slice()] {
            let _ = socket
                .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                .await;
            let _ = socket.write_all(chunk).await;
            let _ = socket.write_all(b"\r\n").await;
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
    });
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .get(format!("http://{addr}/"))
        .send()
        .await
        .unwrap();
    assert!(response.content_length().is_none());
    let error = hopea_node::reach::read_json::<String>(response, 8)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("too large"));
    server.await.unwrap();
}

#[tokio::test]
async fn peer_redirect_cannot_probe_another_service() {
    let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let peer = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = peer.local_addr().unwrap();
    let location = format!("http://{}/private", target.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = peer.accept().await.unwrap();
        let mut request = [0; 4096];
        stream.read(&mut request).await.unwrap();
        stream.write_all(format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let state =
        hopea_node::AppState::new(Config::isolated(dir.path().join("node")), &generate_token())
            .await
            .unwrap();
    assert!(hopea_node::reach::fetch_hello(&state, addr).await.is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), target.accept())
            .await
            .is_err()
    );
    server.await.unwrap();
}
