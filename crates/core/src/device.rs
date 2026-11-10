//! A disk as the writer sees it: sectors that can be written and read back.
//! On Windows that is `\\.\PhysicalDriveN`, in tests a plain file.

use std::fs::File;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use std::os::windows::fs::FileExt;

pub trait BlockDevice {
    /// Bytes per sector. Offsets and lengths of every read and write are a
    /// multiple of it.
    fn sector_size(&self) -> u32;

    /// Size in bytes
    fn size(&self) -> u64;

    fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()>;

    fn read_at(&mut self, offset: u64, data: &mut [u8]) -> io::Result<()>;

    /// Everything written is on the device when this returns
    fn flush(&mut self) -> io::Result<()>;
}

/// A file pretending to be a disk, for tests and for writing an image file
pub struct FileDevice {
    file: File,
    size: u64,
    sector_size: u32,
}

impl FileDevice {
    pub fn new(file: File, size: u64, sector_size: u32) -> io::Result<Self> {
        file.set_len(size)?;
        Ok(Self {
            file,
            size,
            sector_size,
        })
    }
}

impl BlockDevice for FileDevice {
    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        check_bounds(self.size, offset, data.len())?;
        #[cfg(unix)]
        return self.file.write_all_at(data, offset);
        #[cfg(windows)]
        {
            let mut done = 0;
            while done < data.len() {
                done += self.file.seek_write(&data[done..], offset + done as u64)?;
            }
            Ok(())
        }
    }

    fn read_at(&mut self, offset: u64, data: &mut [u8]) -> io::Result<()> {
        check_bounds(self.size, offset, data.len())?;
        #[cfg(unix)]
        return self.file.read_exact_at(data, offset);
        #[cfg(windows)]
        {
            let mut done = 0;
            while done < data.len() {
                let n = self
                    .file
                    .seek_read(&mut data[done..], offset + done as u64)?;
                if n == 0 {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                done += n;
            }
            Ok(())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
}

pub(crate) fn check_bounds(size: u64, offset: u64, len: usize) -> io::Result<()> {
    if offset.checked_add(len as u64).is_none_or(|end| end > size) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{len} bytes at offset {offset} are beyond the end of the disk ({size} bytes)"),
        ));
    }
    Ok(())
}
