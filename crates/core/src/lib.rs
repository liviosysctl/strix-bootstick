//! Everything the bootstick app does, without its window.
//!
//! 1. [`manifest`]: fetch `manifest.json` of the latest release and check it.
//! 2. [`download`]: download the ISO (or its parts) and check every SHA256.
//! 3. [`disks`]: find the USB sticks that may be written, never the Windows disk.
//! 4. [`writer`]: write the image raw, the way Rufus does in DD mode, then read
//!    it back and compare the SHA256.
//! 5. [`boot_entry`]: a firmware boot entry for the stick and `BootNext`, so the
//!    next start boots from the stick without the firmware menu.
//!
//! The parts that touch hardware only exist on Windows (`windows` module). The
//! rest is plain Rust and tested on any system.

pub mod boot_entry;
pub mod device;
pub mod disks;
pub mod download;
pub mod image;
pub mod manifest;
pub mod partition;
pub mod progress;
pub mod release;
pub mod writer;

#[cfg(windows)]
pub mod windows;

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(#[from] Box<ureq::Error>),

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },

    #[error("manifest.json is not valid: {0}")]
    Manifest(String),

    #[error(
        "this app is too old for the release (manifest format {found}, the app knows {known}). Download the newest version of the app"
    )]
    ManifestFormat { found: u32, known: u32 },

    #[error("{file}: the SHA256 does not match (expected {expected}, got {actual})")]
    Checksum {
        file: String,
        expected: String,
        actual: String,
    },

    #[error("{file}: expected {expected} bytes, got {actual}")]
    Size {
        file: String,
        expected: u64,
        actual: u64,
    },

    #[error("the stick is too small: {needed} bytes needed, it has {available}")]
    TooSmall { needed: u64, available: u64 },

    #[error("the image has no EFI system partition")]
    NoEfiPartition,

    #[error("{0}")]
    Refused(String),

    #[error("cancelled")]
    Cancelled,

    #[cfg(windows)]
    #[error("{context}: {source}")]
    Windows {
        context: String,
        #[source]
        source: ::windows::core::Error,
    },
}

impl From<ureq::Error> for Error {
    fn from(error: ureq::Error) -> Self {
        Self::Http(Box::new(error))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Adds what was being done to an I/O error
pub(crate) trait IoContext<T> {
    fn context(self, context: impl FnOnce() -> String) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn context(self, context: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|source| Error::Io {
            context: context(),
            source,
        })
    }
}
