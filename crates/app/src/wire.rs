//! What the work gives the window: the release, the sticks, progress, how a
//! run ended. Plain data, so it is tested on any system; texts are codes the
//! window turns into English or German.

use bootstick_core::Error;
use bootstick_core::disks::{Disk, Unwritable};
use bootstick_core::manifest::Manifest;
use bootstick_core::progress::Stage;

/// The release the stick will hold
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Why a listed stick cannot be chosen
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    TooSmall,
    TooLarge,
}

/// A USB stick or SD card the window lists
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stick {
    /// N in `\\.\PhysicalDriveN`
    pub number: u32,
    /// Vendor and product, empty when the stick names neither
    pub name: String,
    pub size: u64,
    pub letters: Vec<String>,
    pub problem: Option<Problem>,
}

// Used by the Windows machine; the Linux simulation makes its sticks up
#[cfg_attr(not(windows), allow(dead_code))]
impl Stick {
    /// `None` for everything that is not a stick: those disks are not shown
    /// at all, least of all the one Windows runs from.
    pub fn from_disk(disk: &Disk, image_len: u64) -> Option<Self> {
        if !disk.is_stick() {
            return None;
        }
        let problem = match disk.unwritable(image_len) {
            None => None,
            Some(Unwritable::TooSmall) => Some(Problem::TooSmall),
            Some(Unwritable::TooLarge) => Some(Problem::TooLarge),
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

/// What the app was doing when something went wrong
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Reading the sticks only fails on Windows; the simulation never does
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Doing {
    Manifest,
    Sticks,
    Download,
    Write,
    Verify,
    Restart,
}

impl Doing {
    pub fn code(self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::Sticks => "sticks",
            Self::Download => "download",
            Self::Write => "write",
            Self::Verify => "verify",
            Self::Restart => "restart",
        }
    }

    /// The step beside the panel that this belongs to, 0 to 3
    pub fn step(self) -> i32 {
        match self {
            Self::Manifest | Self::Sticks => 0,
            Self::Download => 1,
            Self::Write => 2,
            Self::Verify | Self::Restart => 3,
        }
    }
}

impl From<Stage> for Doing {
    fn from(stage: Stage) -> Self {
        match stage {
            Stage::Download => Self::Download,
            Stage::Write => Self::Write,
            Stage::Verify => Self::Verify,
        }
    }
}

/// Something went wrong
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// What kind, so the window can say it in its language
    pub kind: &'static str,
    pub doing: Doing,
    /// The message of the core, in English, for the small print
    pub detail: String,
}

impl Failure {
    pub fn new(error: &Error, doing: Doing) -> Self {
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
            doing,
            detail: error.to_string(),
        }
    }
}

/// How a run ended
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The stick is written and checked
    Done {
        /// Why the next start of this PC does not go to the stick by itself:
        /// `not-uefi`, `sector-size` or `firmware`; `None` when it does
        no_boot_next: Option<&'static str>,
    },
    Cancelled {
        during: Doing,
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
            Some(Problem::TooSmall)
        );
        let backup = Disk {
            size: 2_000_000_000_000,
            ..disk()
        };
        assert_eq!(
            Stick::from_disk(&backup, iso).unwrap().problem,
            Some(Problem::TooLarge)
        );
    }

    #[test]
    fn failures_have_a_kind_and_a_step() {
        let failure = Failure::new(
            &Error::Checksum {
                file: "the stick".into(),
                expected: "a".into(),
                actual: "b".into(),
            },
            Stage::Verify.into(),
        );
        assert_eq!(failure.kind, "damaged");
        assert_eq!(failure.doing.code(), "verify");
        assert_eq!(failure.doing.step(), 3);
        assert_eq!(Doing::Manifest.step(), 0);
        assert_eq!(Doing::from(Stage::Write).step(), 2);
    }

    #[test]
    fn release_from_manifest() {
        let manifest = Manifest::parse(
            br#"{"format": 1, "name": "Strix", "version": "1.2.0", "released": "2026-11-24T12:51:49Z",
            "iso": {"file": "strix-1.2.0-x86_64.iso", "size": 2967011328,
            "sha256": "92ef906ab017ec32cbf2c2fb8b9f52560a1e86a800baa78a807c37f907534865"},
            "requirements": {"uefi": true, "secureBootOff": false, "ramGiB": 4, "diskGiB": 32}}"#,
        )
        .unwrap();
        assert_eq!(
            Release::from(&manifest),
            Release {
                name: "Strix".into(),
                version: "1.2.0".into(),
                size: 2_967_011_328,
                secure_boot_off: false,
            }
        );
    }
}
