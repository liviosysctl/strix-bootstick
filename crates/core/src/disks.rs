//! Which disks may be written. The rules are here and tested on any system;
//! finding the disks is the Windows part ([`crate::windows::disks`]).
//!
//! The same idea as the disk list of the Strix installer: offer only what is
//! clearly a USB stick, never the disk Windows runs from, never anything that
//! looks like an external backup drive.

use crate::{Error, Result};

/// Larger USB disks are most likely backup drives, not sticks
pub const MAX_SIZE: u64 = 256 * 1000 * 1000 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bus {
    Usb,
    /// SD card readers, built in or on USB
    Sd,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    /// N in `\\.\PhysicalDriveN`
    pub number: u32,
    pub size: u64,
    pub sector_size: u32,
    pub bus: Bus,
    pub removable: bool,
    pub vendor: String,
    pub product: String,
    /// Holds Windows, the boot loader or the page file
    pub system: bool,
    /// Drive letters of its volumes, e.g. `E:`
    pub letters: Vec<String>,
}

/// Why a disk is not written
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unwritable {
    /// Holds Windows
    System,
    /// Not on the USB or SD bus
    NotUsb,
    /// Larger than [`MAX_SIZE`]
    TooLarge,
    /// Smaller than the image
    TooSmall,
}

impl Disk {
    /// For example `SanDisk Ultra, 32 GB (E:)`
    #[must_use]
    pub fn label(&self) -> String {
        let name = format!("{} {}", self.vendor.trim(), self.product.trim());
        let name = name.trim();
        let name = if name.is_empty() { "USB stick" } else { name };
        let letters = if self.letters.is_empty() {
            String::new()
        } else {
            format!(" ({})", self.letters.join(", "))
        };
        format!("{name}, {}{letters}", human_size(self.size))
    }

    /// Whether this is a stick at all: on the USB or SD bus and not the disk
    /// Windows runs from. Everything else is never shown to the user.
    #[must_use]
    pub fn is_stick(&self) -> bool {
        !self.system && matches!(self.bus, Bus::Usb | Bus::Sd)
    }

    /// Why the app does not write `image_len` bytes to this disk, if it does not
    #[must_use]
    pub fn unwritable(&self, image_len: u64) -> Option<Unwritable> {
        if self.system {
            Some(Unwritable::System)
        } else if !matches!(self.bus, Bus::Usb | Bus::Sd) {
            Some(Unwritable::NotUsb)
        } else if self.size > MAX_SIZE {
            Some(Unwritable::TooLarge)
        } else if self.size < image_len {
            Some(Unwritable::TooSmall)
        } else {
            None
        }
    }

    /// Whether the app may write `image_len` bytes to this disk, and why not
    pub fn check_writable(&self, image_len: u64) -> Result<()> {
        match self.unwritable(image_len) {
            None => Ok(()),
            Some(Unwritable::System) => Err(Error::Refused(format!(
                "{} holds Windows and is never written",
                self.label()
            ))),
            Some(Unwritable::NotUsb) => Err(Error::Refused(format!(
                "{} is not a USB stick",
                self.label()
            ))),
            Some(Unwritable::TooLarge) => Err(Error::Refused(format!(
                "{} is larger than {}, probably not a stick but a backup drive",
                self.label(),
                human_size(MAX_SIZE)
            ))),
            Some(Unwritable::TooSmall) => Err(Error::TooSmall {
                needed: image_len,
                available: self.size,
            }),
        }
    }
}

/// Sizes the way stick packages print them: 1 GB = 1000^3 bytes
#[must_use]
pub fn human_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let gb = bytes as f64 / 1e9;
    if gb >= 10.0 {
        format!("{gb:.0} GB")
    } else {
        format!("{gb:.1} GB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stick() -> Disk {
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
    fn label() {
        assert_eq!(stick().label(), "SanDisk Ultra, 32 GB (E:)");
        let anonymous = Disk {
            vendor: String::new(),
            product: String::new(),
            letters: vec![],
            size: 4_000_000_000,
            ..stick()
        };
        assert_eq!(anonymous.label(), "USB stick, 4.0 GB");
    }

    #[test]
    fn rules() {
        let iso = 1_613_086_720;
        assert!(stick().check_writable(iso).is_ok());
        assert!(
            Disk {
                system: true,
                ..stick()
            }
            .check_writable(iso)
            .is_err()
        );
        assert!(
            Disk {
                bus: Bus::Other("NVMe".into()),
                ..stick()
            }
            .check_writable(iso)
            .is_err()
        );
        assert!(
            Disk {
                size: 2_000_000_000_000,
                ..stick()
            }
            .check_writable(iso)
            .is_err()
        );
        assert!(matches!(
            Disk {
                size: 1_000_000_000,
                ..stick()
            }
            .check_writable(iso),
            Err(Error::TooSmall { .. })
        ));
        assert!(
            Disk {
                bus: Bus::Sd,
                ..stick()
            }
            .check_writable(iso)
            .is_ok()
        );
    }

    #[test]
    fn reasons() {
        let iso = 1_613_086_720;
        assert_eq!(stick().unwritable(iso), None);
        assert!(stick().is_stick());
        let windows = Disk {
            system: true,
            ..stick()
        };
        assert_eq!(windows.unwritable(iso), Some(Unwritable::System));
        assert!(!windows.is_stick());
        let internal = Disk {
            bus: Bus::Other("NVMe".into()),
            ..stick()
        };
        assert_eq!(internal.unwritable(iso), Some(Unwritable::NotUsb));
        assert!(!internal.is_stick());
        let backup = Disk {
            size: 2_000_000_000_000,
            ..stick()
        };
        assert_eq!(backup.unwritable(iso), Some(Unwritable::TooLarge));
        assert!(backup.is_stick());
        let tiny = Disk {
            size: 1_000_000_000,
            ..stick()
        };
        assert_eq!(tiny.unwritable(iso), Some(Unwritable::TooSmall));
    }
}
