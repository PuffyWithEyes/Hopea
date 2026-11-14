// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: Apache-2.0

/// Apply an explicit interface constraint only where reqwest can enforce it.
/// A missing constraint works on every platform, including BSD.
pub(crate) fn bind_interface(
    builder: reqwest::ClientBuilder,
    interface: Option<&str>,
) -> anyhow::Result<reqwest::ClientBuilder> {
    let Some(interface) = interface else {
        return Ok(builder);
    };

    // Match ClientBuilder::interface in reqwest 0.13.
    #[cfg(any(
        target_os = "android",
        target_os = "fuchsia",
        target_os = "illumos",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos",
        target_os = "solaris",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos"
    ))]
    return Ok(builder.interface(interface));

    #[cfg(not(any(
        target_os = "android",
        target_os = "fuchsia",
        target_os = "illumos",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos",
        target_os = "solaris",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos"
    )))]
    anyhow::bail!("HTTP interface binding to {interface:?} is not supported on this platform")
}

#[cfg(test)]
mod tests {
    use super::bind_interface;

    #[test]
    fn no_interface_constraint_is_allowed() {
        assert!(bind_interface(reqwest::Client::builder(), None).is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_retains_interface_constraint() {
        assert!(bind_interface(reqwest::Client::builder(), Some("lo")).is_ok());
    }

    #[cfg(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    #[test]
    fn bsd_does_not_silently_ignore_interface_constraint() {
        let error = bind_interface(reqwest::Client::builder(), Some("lo0")).unwrap_err();
        assert!(error.to_string().contains("not supported"));
    }
}
