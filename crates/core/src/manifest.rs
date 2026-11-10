//! `manifest.json`, the contract with the release workflow of the distro
//! (`.github/scripts/package-iso.sh` in the `StrixOS` repository). Fields only get added there,
//! never renamed; anything incompatible raises `format`.

use serde::Deserialize;

use crate::{Error, Result, release};

/// The format this app understands
pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub name: String,
    pub version: String,
    pub released: String,
    pub iso: Iso,
    pub requirements: Requirements,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Iso {
    pub file: String,
    pub size: u64,
    pub sha256: String,
    /// Empty when the ISO is small enough to be one release file
    #[serde(default)]
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Part {
    pub file: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct Requirements {
    pub uefi: bool,
    pub secure_boot_off: bool,
    #[serde(rename = "ramGiB")]
    pub ram_gib: u32,
    #[serde(rename = "diskGiB")]
    pub disk_gib: u32,
}

/// One file to download: the whole ISO or one of its parts
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

impl Manifest {
    /// Parses and checks a manifest
    pub fn parse(json: &[u8]) -> Result<Self> {
        // The format first, so an incompatible manifest says "update the app"
        // and not some error about a missing field
        #[derive(Deserialize)]
        struct FormatOnly {
            format: u32,
        }
        let format: FormatOnly =
            serde_json::from_slice(json).map_err(|e| Error::Manifest(e.to_string()))?;
        if format.format != FORMAT {
            return Err(Error::ManifestFormat {
                found: format.format,
                known: FORMAT,
            });
        }

        let manifest: Self =
            serde_json::from_slice(json).map_err(|e| Error::Manifest(e.to_string()))?;
        manifest.check()?;
        Ok(manifest)
    }

    /// Fetches the manifest of the newest release
    pub fn fetch_latest(agent: &ureq::Agent) -> Result<Self> {
        let body = agent
            .get(release::latest_manifest_url())
            .call()?
            .body_mut()
            .with_config()
            .limit(1024 * 1024)
            .read_to_vec()?;
        Self::parse(&body)
    }

    /// The files to download, in order
    #[must_use]
    pub fn downloads(&self) -> Vec<Download> {
        if self.iso.parts.is_empty() {
            vec![Download {
                file: self.iso.file.clone(),
                url: release::file_url(&self.version, &self.iso.file),
                size: self.iso.size,
                sha256: self.iso.sha256.clone(),
            }]
        } else {
            self.iso
                .parts
                .iter()
                .map(|part| Download {
                    file: part.file.clone(),
                    url: release::file_url(&self.version, &part.file),
                    size: part.size,
                    sha256: part.sha256.clone(),
                })
                .collect()
        }
    }

    fn check(&self) -> Result<()> {
        let bad = |what: &str| Err(Error::Manifest(what.to_owned()));
        if self.name.trim().is_empty() {
            return bad("the name is empty");
        }
        if !is_version(&self.version) {
            return bad("the version is not x.y.z");
        }
        let mut files = vec![self.iso.file.as_str()];
        files.extend(self.iso.parts.iter().map(|p| p.file.as_str()));
        if files.iter().any(|f| !is_file_name(f)) {
            return bad("a file name contains a path or unexpected characters");
        }
        let mut hashes = vec![self.iso.sha256.as_str()];
        hashes.extend(self.iso.parts.iter().map(|p| p.sha256.as_str()));
        if hashes.iter().any(|h| !is_sha256(h)) {
            return bad("a SHA256 is not 64 hex digits");
        }
        if self.iso.size == 0 {
            return bad("the ISO size is 0");
        }
        if !self.iso.parts.is_empty() {
            let sum: u64 = self.iso.parts.iter().map(|p| p.size).sum();
            if sum != self.iso.size {
                return bad("the parts do not add up to the ISO size");
            }
        }
        Ok(())
    }
}

fn is_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Only plain names: they end up in a URL and in a local path
fn is_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The manifest of v0.1.0 as released on 2026-11-10
    const V010: &str = r#"{
      "format": 1,
      "name": "Strix",
      "version": "0.1.0",
      "released": "2026-11-10T14:02:02Z",
      "iso": {
        "file": "strix-0.1.0-x86_64.iso",
        "size": 1613086720,
        "sha256": "675fec63bcf49bf4a328b36fc068ca5476a5c769a11254642cb7af283b1cbf42",
        "parts": []
      },
      "requirements": { "uefi": true, "secureBootOff": true, "ramGiB": 4, "diskGiB": 32 }
    }"#;

    const SHA: &str = "675fec63bcf49bf4a328b36fc068ca5476a5c769a11254642cb7af283b1cbf42";

    #[test]
    fn real_manifest() {
        let m = Manifest::parse(V010.as_bytes()).unwrap();
        assert_eq!(m.name, "Strix");
        assert_eq!(m.requirements.ram_gib, 4);
        assert!(m.requirements.secure_boot_off);
        let d = m.downloads();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].url,
            "https://github.com/liviosysctl/StrixOS/releases/download/v0.1.0/strix-0.1.0-x86_64.iso"
        );
        assert_eq!(d[0].size, 1_613_086_720);
    }

    #[test]
    fn parts() {
        let json = V010.replace(
            r#""parts": []"#,
            &format!(
                r#""parts": [
                  {{ "file": "a.iso.part00", "size": 1000000000, "sha256": "{SHA}" }},
                  {{ "file": "a.iso.part01", "size": 613086720, "sha256": "{SHA}" }}
                ]"#
            ),
        );
        let m = Manifest::parse(json.as_bytes()).unwrap();
        let d = m.downloads();
        assert_eq!(d.len(), 2);
        assert!(d[1].url.ends_with("/v0.1.0/a.iso.part01"));
    }

    #[test]
    fn parts_must_add_up() {
        let json = V010.replace(
            r#""parts": []"#,
            &format!(r#""parts": [{{ "file": "a.part00", "size": 5, "sha256": "{SHA}" }}]"#),
        );
        assert!(matches!(
            Manifest::parse(json.as_bytes()),
            Err(Error::Manifest(_))
        ));
    }

    #[test]
    fn newer_format_asks_for_an_update() {
        let json = V010.replace(r#""format": 1"#, r#""format": 2, "other": true"#);
        assert!(matches!(
            Manifest::parse(json.as_bytes()),
            Err(Error::ManifestFormat { found: 2, known: 1 })
        ));
    }

    #[test]
    fn unknown_fields_are_fine() {
        let json = V010.replace(r#""format": 1,"#, r#""format": 1, "signed": false,"#);
        assert!(Manifest::parse(json.as_bytes()).is_ok());
    }

    #[test]
    fn file_names_cannot_escape() {
        for name in ["../evil.iso", "a/b.iso", ".hidden", r"a\b.iso", ""] {
            let json = V010.replace("strix-0.1.0-x86_64.iso", name);
            assert!(Manifest::parse(json.as_bytes()).is_err(), "{name}");
        }
    }

    #[test]
    fn bad_hash_and_version() {
        let json = V010.replace(SHA, "abc");
        assert!(Manifest::parse(json.as_bytes()).is_err());
        let json = V010.replace(r#""version": "0.1.0""#, r#""version": "0.1""#);
        assert!(Manifest::parse(json.as_bytes()).is_err());
    }
}
