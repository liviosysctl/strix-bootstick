//! Writing the image raw onto a disk, the way Rufus does it in DD mode, plus a
//! read-back check that Rufus does not do by default.
//!
//! Locking and dismounting the volumes of the stick happens before, in the
//! Windows part ([`crate::windows::device`]). This file only sees sectors:
//!
//! 1. Wipe the first and the last MiB. GPT keeps a backup header at the end of
//!    the disk; left over from an earlier use, firmware and Windows might
//!    believe it over the new image.
//! 2. Write the image in large blocks aligned to the sector size. The last
//!    block is padded with zeros to a full sector. A block that fails is tried
//!    again before giving up.
//! 3. Flush, then read the whole image back and compare its SHA256 with the
//!    one from the manifest.

use std::io::Read;

use sha2::{Digest, Sha256};

use crate::device::BlockDevice;
use crate::progress::{Observer, Progress, Stage, check_cancel};
use crate::{Error, IoContext, Result};

/// Bytes per write. Large blocks are much faster on USB sticks than small
/// ones; 4 MiB is what Rufus uses as well.
pub const BLOCK: usize = 4 << 20;

const WIPE: u64 = 1 << 20;
const ATTEMPTS: u32 = 4;

pub struct Options {
    /// Read everything back and compare the SHA256 after writing
    pub verify: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { verify: true }
    }
}

/// Writes `image` (`image_len` bytes, SHA256 `sha256`) to the start of the
/// device. The image is hashed while it is read, so a broken download is
/// noticed even without the read-back check.
pub fn write_image(
    device: &mut dyn BlockDevice,
    image: &mut dyn Read,
    image_len: u64,
    sha256: &str,
    options: &Options,
    observer: &mut dyn Observer,
) -> Result<()> {
    let sector = u64::from(device.sector_size());
    if sector == 0 || !(BLOCK as u64).is_multiple_of(sector) {
        return Err(Error::Refused(format!("unusual sector size {sector}")));
    }
    let padded_len = image_len.div_ceil(sector) * sector;
    if padded_len > device.size() {
        return Err(Error::TooSmall {
            needed: padded_len,
            available: device.size(),
        });
    }

    wipe_ends(device)?;

    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; BLOCK];
    let mut offset: u64 = 0;
    while offset < image_len {
        check_cancel(observer)?;
        let want = usize::try_from((image_len - offset).min(BLOCK as u64)).unwrap_or(BLOCK);
        read_full(image, &mut buffer[..want])?;
        hasher.update(&buffer[..want]);
        let len = (want as u64).div_ceil(sector) * sector;
        let len = usize::try_from(len).unwrap_or(BLOCK);
        buffer[want..len].fill(0);
        write_with_retry(device, offset, &buffer[..len])?;
        offset += want as u64;
        observer.progress(&Progress {
            stage: Stage::Write,
            done: offset,
            total: image_len,
            file: String::new(),
        });
    }
    let mut rest = [0u8; 1];
    if image
        .read(&mut rest)
        .context(|| "read the image".to_owned())?
        != 0
    {
        return Err(Error::Refused(
            "the image is longer than expected".to_owned(),
        ));
    }
    check_hash("image", sha256, &hex::encode(hasher.finalize()))?;
    device.flush().context(|| "flush the stick".to_owned())?;

    if options.verify {
        verify(device, image_len, sha256, observer)?;
    }
    Ok(())
}

/// Reads the first `len` bytes back and compares their SHA256
pub fn verify(
    device: &mut dyn BlockDevice,
    len: u64,
    sha256: &str,
    observer: &mut dyn Observer,
) -> Result<()> {
    let sector = u64::from(device.sector_size());
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; BLOCK];
    let mut offset = 0;
    while offset < len {
        check_cancel(observer)?;
        let want = usize::try_from((len - offset).min(BLOCK as u64)).unwrap_or(BLOCK);
        let read_len = usize::try_from((want as u64).div_ceil(sector) * sector).unwrap_or(BLOCK);
        device
            .read_at(offset, &mut buffer[..read_len])
            .context(|| format!("read back at byte {offset}"))?;
        hasher.update(&buffer[..want]);
        offset += want as u64;
        observer.progress(&Progress {
            stage: Stage::Verify,
            done: offset,
            total: len,
            file: String::new(),
        });
    }
    check_hash("the stick", sha256, &hex::encode(hasher.finalize()))
}

fn wipe_ends(device: &mut dyn BlockDevice) -> Result<()> {
    let size = device.size();
    let sector = u64::from(device.sector_size());
    let wipe = WIPE.min(size);
    let zeros = vec![0u8; usize::try_from(wipe).unwrap_or(BLOCK)];
    write_with_retry(device, 0, &zeros)?;
    let end_start = (size - wipe) / sector * sector;
    let end_len = usize::try_from(size - end_start).unwrap_or(zeros.len());
    write_with_retry(device, end_start, &zeros[..end_len.min(zeros.len())])
}

fn write_with_retry(device: &mut dyn BlockDevice, offset: u64, data: &[u8]) -> Result<()> {
    let mut last = None;
    for _ in 0..ATTEMPTS {
        match device.write_at(offset, data) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err(Error::Io {
        context: format!("write {} bytes at byte {offset}", data.len()),
        source: last.unwrap_or_else(|| std::io::ErrorKind::Other.into()),
    })
}

fn read_full(image: &mut dyn Read, buffer: &mut [u8]) -> Result<()> {
    image.read_exact(buffer).map_err(|source| Error::Io {
        context: "read the image (shorter than the manifest says?)".to_owned(),
        source,
    })
}

fn check_hash(what: &str, expected: &str, actual: &str) -> Result<()> {
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(Error::Checksum {
            file: what.to_owned(),
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;
    use crate::device::FileDevice;
    use crate::progress::Silent;

    fn sha(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    fn file_device(size: u64, sector: u32) -> (tempfile::NamedTempFile, FileDevice) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = tmp.reopen().unwrap();
        // Old content everywhere, like a used stick
        std::fs::write(tmp.path(), vec![0xAA; usize::try_from(size).unwrap()]).unwrap();
        let device = FileDevice::new(file, size, sector).unwrap();
        (tmp, device)
    }

    #[test]
    fn writes_verifies_and_wipes_the_end() {
        let size = 16 << 20;
        let (tmp, mut device) = file_device(size, 512);
        // Not a multiple of the block or the sector size
        let image: Vec<u8> = (0..(5 << 20) + 1000)
            .map(|i: u32| (i % 251) as u8)
            .collect();
        write_image(
            &mut device,
            &mut image.as_slice(),
            image.len() as u64,
            &sha(&image),
            &Options::default(),
            &mut Silent,
        )
        .unwrap();
        let disk = std::fs::read(tmp.path()).unwrap();
        assert_eq!(&disk[..image.len()], &image[..]);
        // Padding up to the sector boundary is zero
        let padded = image.len().div_ceil(512) * 512;
        assert!(disk[image.len()..padded].iter().all(|&b| b == 0));
        // Old content between image and end stays, the last MiB is wiped
        assert_eq!(disk[padded + 10], 0xAA);
        assert!(disk[disk.len() - (1 << 20)..].iter().all(|&b| b == 0));
    }

    #[test]
    fn wrong_hash_is_an_error() {
        let (_tmp, mut device) = file_device(8 << 20, 512);
        let image = vec![1u8; 4096];
        let result = write_image(
            &mut device,
            &mut image.as_slice(),
            4096,
            &"0".repeat(64),
            &Options::default(),
            &mut Silent,
        );
        assert!(matches!(result, Err(Error::Checksum { .. })));
    }

    #[test]
    fn too_small() {
        let (_tmp, mut device) = file_device(1 << 20, 512);
        let image = vec![1u8; 2 << 20];
        let result = write_image(
            &mut device,
            &mut image.as_slice(),
            image.len() as u64,
            &sha(&image),
            &Options::default(),
            &mut Silent,
        );
        assert!(matches!(result, Err(Error::TooSmall { .. })));
    }

    /// Fails the first write at each offset, like a flaky USB connection
    struct Flaky {
        inner: FileDevice,
        failed: std::collections::HashSet<u64>,
    }

    impl BlockDevice for Flaky {
        fn sector_size(&self) -> u32 {
            self.inner.sector_size()
        }
        fn size(&self) -> u64 {
            self.inner.size()
        }
        fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
            if self.failed.insert(offset) {
                return Err(io::Error::other("flaky"));
            }
            self.inner.write_at(offset, data)
        }
        fn read_at(&mut self, offset: u64, data: &mut [u8]) -> io::Result<()> {
            self.inner.read_at(offset, data)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    #[test]
    fn retries_failed_blocks() {
        let (_tmp, inner) = file_device(16 << 20, 4096);
        let mut device = Flaky {
            inner,
            failed: std::collections::HashSet::new(),
        };
        let image = vec![3u8; 9 << 20];
        write_image(
            &mut device,
            &mut image.as_slice(),
            image.len() as u64,
            &sha(&image),
            &Options::default(),
            &mut Silent,
        )
        .unwrap();
    }

    #[test]
    fn cancel_stops() {
        struct Stop;
        impl Observer for Stop {
            fn progress(&mut self, _: &Progress) {}
            fn cancelled(&self) -> bool {
                true
            }
        }
        let (_tmp, mut device) = file_device(8 << 20, 512);
        let image = vec![1u8; 4096];
        let result = write_image(
            &mut device,
            &mut image.as_slice(),
            4096,
            &sha(&image),
            &Options::default(),
            &mut Stop,
        );
        assert!(matches!(result, Err(Error::Cancelled)));
    }
}
