// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Android lyrics transport, independent of GTK/GIO's optional TLS modules.
//! The public root certificates are bundled with the APK. Both certificate
//! and hostname verification stay enabled, including after redirects.

use std::time::Duration;

#[derive(Debug)]
pub enum FetchError {
    Transport(reqwest::Error),
    UnexpectedStatus,
    TooLarge,
}

impl From<reqwest::Error> for FetchError {
    fn from(error: reqwest::Error) -> Self {
        Self::Transport(error)
    }
}

fn client_builder(user_agent: &str) -> Result<reqwest::ClientBuilder, FetchError> {
    let roots = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|cert| reqwest::Certificate::from_der(cert.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(reqwest::Client::builder()
        .tls_backend_rustls()
        // Reqwest's default platform verifier requires an extra Java component
        // and JNI initialization on Android. Supplying public roots explicitly
        // uses Rustls's WebPKI verifier without either runtime dependency.
        .tls_certs_only(roots)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(3))
        .timeout(Duration::from_secs(20))
        .user_agent(user_agent))
}

pub async fn fetch(
    url: &str,
    user_agent: &str,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, FetchError> {
    let client = client_builder(user_agent)?.build()?;
    fetch_with_client(&client, url, max_bytes).await
}

async fn fetch_with_client(
    client: &reqwest::Client,
    url: &str,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, FetchError> {
    let mut response = client.get(url).send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(FetchError::UnexpectedStatus);
    }
    if response
        .content_length()
        .is_some_and(|n| n > max_bytes as u64)
    {
        return Err(FetchError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        // Enforce the limit on actual bytes too: chunked replies may have no
        // Content-Length. Do not allocate an oversized response first.
        if chunk.len() > max_bytes.saturating_sub(body.len()) {
            return Err(FetchError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

#[cfg(test)]
#[path = "../tests/support/lyrics_https.rs"]
mod tests;
