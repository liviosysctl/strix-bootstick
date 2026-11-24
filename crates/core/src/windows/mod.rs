//! The parts that talk to Windows: finding disks, writing to a whole disk and
//! the firmware boot entry. Everything needs administrator rights except
//! [`disks::list`] and [`system`].

pub mod device;
pub mod disks;
pub mod firmware;
pub mod system;

use std::path::PathBuf;

use ::windows::Win32::Foundation::{CloseHandle, HANDLE};
use ::windows::core::PCWSTR;

use crate::image::Image;
use crate::partition::{self, HEAD_BYTES};
use crate::progress::Observer;
use crate::writer::{self, Options};
use crate::{Error, Result};

/// What [`flash`] should do
pub struct Flash<'a> {
    pub disk: u32,
    /// The downloaded ISO, in parts or as one file
    pub image: &'a [PathBuf],
    pub sha256: &'a str,
    /// Read the stick back and compare the SHA256
    pub verify: bool,
    /// Set a boot entry and `BootNext`, with this description (e.g. "Strix
    /// installer"). `None` leaves the firmware alone.
    pub boot_entry: Option<&'a str>,
    /// Only check and report, write nothing
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub disk: String,
    pub bytes: u64,
    /// The number of the boot entry if one was set
    pub boot_entry: Option<u16>,
    /// Why no boot entry was set although one was asked for
    pub no_boot_entry: Option<NoBootEntry>,
}

/// Why the next start does not go to the stick by itself. The stick is fine
/// either way; the user picks it in the boot menu instead.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NoBootEntry {
    #[error("this PC does not start with UEFI")]
    NotUefi,
    /// The partition table of the image counts 512-byte sectors
    #[error("the stick has {0}-byte sectors")]
    SectorSize(u32),
    /// Some firmware refuses new entries or `BootNext`
    #[error("the firmware did not take the boot entry ({0})")]
    Firmware(String),
}

/// Writes the image to the disk and prepares the next start from it. Checks
/// the disk again right before writing: the list the user picked from may be
/// minutes old.
pub fn flash(job: &Flash, observer: &mut dyn Observer) -> Result<Report> {
    let disk = disks::list()?
        .into_iter()
        .find(|d| d.number == job.disk)
        .ok_or_else(|| Error::Refused(format!("disk {} is gone", job.disk)))?;
    let image = Image::open(job.image)?;
    let image_len = image.len();
    disk.check_writable(image_len)?;

    let head = Image::head(job.image, HEAD_BYTES)?;
    let esp = partition::find_efi_partition(&head)?;

    let mut report = Report {
        disk: disk.label(),
        bytes: image_len,
        boot_entry: None,
        no_boot_entry: None,
    };
    if job.dry_run {
        return Ok(report);
    }

    let mut drive = device::PhysicalDrive::open_for_writing(&disk)?;
    let mut image = image;
    let written = writer::write_image(
        &mut drive,
        &mut image,
        image_len,
        job.sha256,
        &Options { verify: job.verify },
        observer,
    );
    drive.finish()?;
    written?;

    if let Some(description) = job.boot_entry {
        if disk.sector_size != 512 {
            report.no_boot_entry = Some(NoBootEntry::SectorSize(disk.sector_size));
        } else if !firmware::is_uefi() {
            report.no_boot_entry = Some(NoBootEntry::NotUefi);
        } else {
            // The stick is written and checked at this point: a firmware that
            // refuses the entry must not turn that into a failure
            match firmware::boot_next_once(description, &esp) {
                Ok(number) => report.boot_entry = Some(number),
                Err(error) => {
                    report.no_boot_entry = Some(NoBootEntry::Firmware(error.to_string()));
                }
            }
        }
    }
    Ok(report)
}

/// A handle that is closed when dropped
pub(crate) struct Handle(pub HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: the handle came from CreateFileW and is closed only here
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// A null-terminated UTF-16 string for the Win32 API
pub(crate) struct Wide(Vec<u16>);

impl Wide {
    pub fn new(text: &str) -> Self {
        Self(text.encode_utf16().chain(std::iter::once(0)).collect())
    }

    pub fn pcwstr(&self) -> PCWSTR {
        PCWSTR(self.0.as_ptr())
    }
}

pub(crate) fn win(context: impl Into<String>) -> impl FnOnce(::windows::core::Error) -> Error {
    let context = context.into();
    move |source| Error::Windows { context, source }
}
