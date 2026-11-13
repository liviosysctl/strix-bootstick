//! Where releases come from. The app has no distro name built in: the name it
//! shows comes from `manifest.json`.

use std::time::Duration;

/// GitHub repository with the ISO
pub const OWNER: &str = "liviosysctl";
pub const REPO: &str = "StrixOS";

/// The pre-release of the proof of concept. CI on the branch m245-poc builds
/// and signs the ISO on every push and replaces the files of this one
/// release, so its URLs never change. The real releases of the distribution
/// (vX.Y.Z) carry no manifest.
pub const TAG: &str = "poc";

/// `manifest.json` of the proof of concept. This URL redirects to the file
/// and does not use the GitHub API, so the limit of 60 API calls per hour and
/// address does not apply (a classroom shares one address).
#[must_use]
pub fn manifest_url() -> String {
    format!("https://github.com/{OWNER}/{REPO}/releases/download/{TAG}/manifest.json")
}

/// A file of the proof of concept's release
#[must_use]
pub fn file_url(file: &str) -> String {
    format!("https://github.com/{OWNER}/{REPO}/releases/download/{TAG}/{file}")
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
            manifest_url(),
            "https://github.com/liviosysctl/StrixOS/releases/download/poc/manifest.json"
        );
        assert_eq!(
            file_url("strix-0.2.0-x86_64.iso"),
            "https://github.com/liviosysctl/StrixOS/releases/download/poc/strix-0.2.0-x86_64.iso"
        );
    }
}
