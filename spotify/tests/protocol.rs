// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Uses synthetic credentials and local fixtures; never contacts Spotify.
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

fn run(input: &str) -> Output {
    run_with_path(input, None)
}

fn run_with_path(input: &str, path: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hopea-spotify"));
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("worker did not exit after EOF");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn no_selection_does_not_connect_or_persist_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let output = run(&format!(
        "{}\n",
        json!({"secret": "synthetic-expired-token", "directory": dir.path()})
    ));
    assert!(output.status.success());
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn existing_audio_is_reused_without_authentication_or_redownload() {
    let dir = tempfile::tempdir().unwrap();
    let key = "4uLU6hMCjMI75M1A2tKUQC";
    let path = dir.path().join(format!(
        "{}.flac",
        hex::encode(Sha256::digest(format!("spotify:{key}")))
    ));
    let audio = include_bytes!("../../node/tests/fixtures/tone.flac");
    std::fs::write(&path, audio).unwrap();
    let input = format!(
        "{}\n{}\n",
        json!({"secret": "synthetic-expired-token", "directory": dir.path()}),
        json!({"key": key, "title": "Test", "collections": []})
    );
    let output = run(&input);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["event"], "downloaded");
    assert_eq!(result["key"], key);
    assert_eq!(result["path"], path.to_str().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), audio);
    assert!(output.stderr.is_empty());
}

#[test]
fn malformed_requests_do_not_echo_secrets() {
    let output = run("{\"secret\":\"never-echo-this-token\",\"directory\":\"relative\"}\n");
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("never-echo-this-token"));
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["event"], "error");
}

#[test]
fn local_playlist_entries_remain_metadata_without_contacting_spotify() {
    let dir = tempfile::tempdir().unwrap();
    let key = "spotify:local:Artist:Album:Title:123";
    let input = format!(
        "{}\n{}\n",
        json!({"secret": "synthetic-token", "directory": dir.path()}),
        json!({"key": key, "title": "Title", "collections": []})
    );
    let output = run_with_path(&input, Some("/nonexistent/hopea-test-path"));
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["key"], key);
    assert!(result["path"].is_null());
}

#[test]
fn missing_converter_is_reported_before_attempting_sign_in() {
    let dir = tempfile::tempdir().unwrap();
    let input = format!(
        "{}\n{}\n",
        json!({"secret": "synthetic-token", "directory": dir.path()}),
        json!({"key": "4uLU6hMCjMI75M1A2tKUQC", "title": "Title", "collections": []})
    );
    let output = run_with_path(&input, Some("/nonexistent/hopea-test-path"));
    assert!(!output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result["message"]
        .as_str()
        .unwrap()
        .contains("FFmpeg and FFprobe"));
    assert!(output.stderr.is_empty());
}

#[cfg(unix)]
#[test]
fn cancellation_stops_a_worker_waiting_for_more_commands() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hopea-spotify"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(
        input,
        "{}",
        json!({"secret": "synthetic-token", "directory": dir.path()})
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    // The PID is a separate argument, not interpolated shell code.
    assert!(Command::new("sh")
        .args([
            "-c",
            "kill -TERM \"$1\"",
            "cancel-worker",
            &child.id().to_string()
        ])
        .status()
        .unwrap()
        .success());
    let deadline = Instant::now() + Duration::from_secs(4);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("cancellation left the worker running");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "SIGTERM should shut the worker down cleanly"
    );
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
