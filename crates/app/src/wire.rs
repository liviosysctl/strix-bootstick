//! What crosses between the Rust side and the page, as JSON. The page shows
//! every text itself (it has the languages), so nothing here is a sentence:
//! reasons and failures are short codes, plus the English detail of the core
//! for the small print.

use bootstick_core::Error;
use bootstick_core::disks::{Disk, Unwritable};
use bootstick_core::manifest::Manifest;
use bootstick_core::progress::{Progress, Stage};
use serde::Serialize;

/// The release the stick will hold
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub name: String,
    pub version: String,
    /// Bytes to download and to write
    pub size: u64,
    /// The unsigned ISO only starts with Secure Boot off
    pub secure_boot_off: bool,
}

impl From<&Manifest> for Release {
    fn from(manifest: &Manifest) -> Self {
        Self {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            size: manifest.iso.size,
            secure_boot_off: manifest.requirements.secure_boot_off,
        }
    }
}

/// A USB stick or SD card the page lists
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stick {
    /// N in `\\.\PhysicalDriveN`, what `start` takes
    pub number: u32,
    /// Vendor and product, empty when the stick names neither
    pub name: String,
    pub size: u64,
    pub letters: Vec<String>,
    /// Why it cannot be chosen: `too-small` or `too-large`
    pub problem: Option<&'static str>,
}

impl Stick {
    /// `None` for everything that is not a stick: those disks are not shown
    /// at all, least of all the one Windows runs from.
    pub fn from_disk(disk: &Disk, image_len: u64) -> Option<Self> {
        if !disk.is_stick() {
            return None;
        }
        let problem = match disk.unwritable(image_len) {
            None => None,
            Some(Unwritable::TooSmall) => Some("too-small"),
            Some(Unwritable::TooLarge) => Some("too-large"),
            Some(Unwritable::System | Unwritable::NotUsb) => return None,
        };
        Some(Self {
            number: disk.number,
            name: format!("{} {}", disk.vendor.trim(), disk.product.trim())
                .trim()
                .to_owned(),
            size: disk.size,
            letters: disk.letters.clone(),
            problem,
        })
    }
}

pub fn stage_code(stage: Stage) -> &'static str {
    match stage {
        Stage::Download => "download",
        Stage::Write => "write",
        Stage::Verify => "verify",
    }
}

/// One progress report
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub stage: &'static str,
    pub done: u64,
    pub total: u64,
}

impl From<&Progress> for Step {
    fn from(progress: &Progress) -> Self {
        Self {
            stage: stage_code(progress.stage),
            done: progress.done,
            total: progress.total,
        }
    }
}

/// Something went wrong
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    /// What kind, so the page can say it in its language
    pub kind: &'static str,
    /// What the app was doing: `manifest`, `sticks`, `download`, `write`,
    /// `verify` or `restart`
    pub stage: &'static str,
    /// The message of the core, in English
    pub detail: String,
}

impl Failure {
    pub fn new(error: &Error, stage: &'static str) -> Self {
        let kind = match error {
            Error::Http(_) => "network",
            Error::Io { .. } => "io",
            Error::Manifest(_) => "manifest",
            Error::ManifestFormat { .. } => "outdated",
            Error::Checksum { .. } | Error::Size { .. } => "damaged",
            Error::TooSmall { .. } => "too-small",
            Error::NoEfiPartition => "image",
            Error::Refused(_) => "refused",
            Error::Cancelled => "cancelled",
            #[cfg(windows)]
            Error::Windows { .. } => "windows",
        };
        Self {
            kind,
            stage,
            detail: error.to_string(),
        }
    }

    pub fn during(error: &Error, stage: Stage) -> Self {
        Self::new(error, stage_code(stage))
    }
}

/// How a run ended
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum Outcome {
    /// The stick is written and checked
    #[serde(rename_all = "camelCase")]
    Done {
        /// The next start of this PC goes to the stick by itself
        boot_next: bool,
        /// Why not: `not-uefi`, `sector-size` or `firmware`
        no_boot_next: Option<&'static str>,
    },
    Cancelled {
        stage: &'static str,
    },
    Failed(Failure),
}

#[cfg(test)]
mod tests {
    use bootstick_core::disks::Bus;

    use super::*;

    fn disk() -> Disk {
        Disk {
            number: 2,
            size: 32_017_047_552,
            sector_size: 512,
            bus: Bus::Usb,
            removable: true,
            vendor: "SanDisk ".into(),
            product: "Ultra ".into(),
            system: false,
            letters: vec!["E:".into()],
        }
    }

    #[test]
    fn only_sticks_are_listed() {
        let iso = 2_967_011_328;
        let stick = Stick::from_disk(&disk(), iso).unwrap();
        assert_eq!(stick.name, "SanDisk Ultra");
        assert_eq!(stick.problem, None);

        let windows = Disk {
            system: true,
            ..disk()
        };
        assert_eq!(Stick::from_disk(&windows, iso), None);
        let internal = Disk {
            bus: Bus::Other("NVMe".into()),
            ..disk()
        };
        assert_eq!(Stick::from_disk(&internal, iso), None);

        let tiny = Disk {
            size: 2_000_000_000,
            ..disk()
        };
        assert_eq!(
            Stick::from_disk(&tiny, iso).unwrap().problem,
            Some("too-small")
        );
        let backup = Disk {
            size: 2_000_000_000_000,
            ..disk()
        };
        assert_eq!(
            Stick::from_disk(&backup, iso).unwrap().problem,
            Some("too-large")
        );
    }

    #[test]
    fn json_the_page_reads() {
        let stick = Stick::from_disk(&disk(), 0).unwrap();
        assert_eq!(
            serde_json::to_value(&stick).unwrap(),
            serde_json::json!({
                "number": 2, "name": "SanDisk Ultra", "size": 32_017_047_552_u64,
                "letters": ["E:"], "problem": null
            })
        );

        let done = Outcome::Done {
            boot_next: false,
            no_boot_next: Some("not-uefi"),
        };
        assert_eq!(
            serde_json::to_value(&done).unwrap(),
            serde_json::json!({
                "outcome": "done", "bootNext": false,
                "noBootNext": "not-uefi"
            })
        );

        let failed = Outcome::Failed(Failure::during(
            &Error::Checksum {
                file: "the stick".into(),
                expected: "a".into(),
                actual: "b".into(),
            },
            Stage::Verify,
        ));
        let json = serde_json::to_value(&failed).unwrap();
        assert_eq!(json["outcome"], "failed");
        assert_eq!(json["kind"], "damaged");
        assert_eq!(json["stage"], "verify");

        let step = Step::from(&Progress {
            stage: Stage::Write,
            done: 4 << 20,
            total: 2_967_011_328,
            file: String::new(),
        });
        assert_eq!(
            serde_json::to_value(&step).unwrap(),
            serde_json::json!({"stage": "write", "done": 4_194_304, "total": 2_967_011_328_u64})
        );

        let manifest = Manifest::parse(
            br#"{"format": 1, "name": "Strix", "version": "1.2.0", "released": "2026-11-24T12:51:49Z",
            "iso": {"file": "strix-1.2.0-x86_64.iso", "size": 2967011328,
            "sha256": "92ef906ab017ec32cbf2c2fb8b9f52560a1e86a800baa78a807c37f907534865"},
            "requirements": {"uefi": true, "secureBootOff": false, "ramGiB": 4, "diskGiB": 32}}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(Release::from(&manifest)).unwrap(),
            serde_json::json!({
                "name": "Strix", "version": "1.2.0", "size": 2_967_011_328_u64,
                "secureBootOff": false
            })
        );

        let cancelled = Outcome::Cancelled { stage: "write" };
        assert_eq!(
            serde_json::to_value(&cancelled).unwrap(),
            serde_json::json!({"outcome": "cancelled", "stage": "write"})
        );
    }
}
