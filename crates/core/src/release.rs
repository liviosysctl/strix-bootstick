//! Where releases come from. The app has no distro name built in: the name it
//! shows comes from `manifest.json`.

use std::time::Duration;

/// GitHub repository with the releases
pub const OWNER: &str = "liviosysctl";
pub const REPO: &str = "StrixOS";

/// The newest release that is not a pre-release. This URL redirects to the
/// file and does not use the GitHub API, so the limit of 60 API calls per hour
/// and address does not apply (a classroom shares one address).
#[must_use]
pub fn latest_manifest_url() -> String {
    format!("https://github.com/{OWNER}/{REPO}/releases/latest/download/manifest.json")
}

/// A file of the release with this version
#[must_use]
pub fn file_url(version: &str, file: &str) -> String {
    format!("https://github.com/{OWNER}/{REPO}/releases/download/v{version}/{file}")
}

/// The HTTP client for manifest and downloads. It trusts the certificates of
/// the system, so a school proxy that the browser trusts works here too.
#[must_use]
pub fn agent() -> ureq::Agent {
    #[cfg(windows)]
    let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls);
    // Elsewhere rustls, but with the certificates of the system as well
    #[cfg(not(windows))]
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier);
    ureq::Agent::config_builder()
        .tls_config(tls.build())
        .timeout_connect(Some(Duration::from_secs(20)))
        .user_agent(concat!("bootstick/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            latest_manifest_url(),
            "https://github.com/liviosysctl/StrixOS/releases/latest/download/manifest.json"
        );
        assert_eq!(
            file_url("0.1.0", "strix-0.1.0-x86_64.iso"),
            "https://github.com/liviosysctl/StrixOS/releases/download/v0.1.0/strix-0.1.0-x86_64.iso"
        );
    }
}
