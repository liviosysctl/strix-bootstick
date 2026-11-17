//! Writing to a whole disk, `\\.\PhysicalDriveN`, the way Rufus prepares it:
//!
//! 1. Lock and dismount every volume on the disk, so Explorer, the indexer or
//!    an antivirus cannot write in between. Locking is retried for a few
//!    seconds; something usually holds a freshly plugged stick briefly.
//! 2. Open the disk unbuffered and write-through, allow I/O beyond the old
//!    volume ends (`FSCTL_ALLOW_EXTENDED_DASD_IO`).
//! 3. Delete the partition layout Windows knows, so it does not reread the old
//!    one while the image is written.
//! 4. After writing ([`PhysicalDrive::finish`]): make Windows read the new
//!    partition table, then release the locks.
//!
//! Unbuffered I/O needs buffers aligned to the sector size; the writer's
//! blocks are copied into one aligned buffer.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::io;
use std::time::Duration;

use ::windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
use ::windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FlushFileBuffers, OPEN_EXISTING, ReadFile, WriteFile,
};
use ::windows::Win32::System::IO::{DeviceIoControl, OVERLAPPED};
use ::windows::Win32::System::Ioctl::{
    FSCTL_ALLOW_EXTENDED_DASD_IO, FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME,
    IOCTL_DISK_DELETE_DRIVE_LAYOUT, IOCTL_DISK_UPDATE_PROPERTIES,
};

use super::disks::volumes;
use super::{Handle, Wide, win};
use crate::device::{BlockDevice, check_bounds};
use crate::disks::Disk;
use crate::writer::BLOCK;
use crate::{Error, Result};

const LOCK_ATTEMPTS: u32 = 20;
const LOCK_WAIT: Duration = Duration::from_millis(250);
const ALIGN: usize = 4096;

pub struct PhysicalDrive {
    handle: Handle,
    size: u64,
    sector_size: u32,
    /// Locked volumes; the locks end when the handles close
    _volumes: Vec<Handle>,
    buffer: AlignedBuffer,
}

impl PhysicalDrive {
    /// Locks the disk's volumes and opens it for writing. Needs administrator
    /// rights.
    pub fn open_for_writing(disk: &Disk) -> Result<Self> {
        let mut locked = Vec::new();
        for volume in volumes()?.iter().filter(|v| v.disks.contains(&disk.number)) {
            if volume.disks.len() > 1 {
                return Err(Error::Refused(format!(
                    "{} is part of a volume that spans several disks",
                    disk.label()
                )));
            }
            locked.push(lock_volume(volume.name.trim_end_matches('\\'))?);
        }

        let path = format!(r"\\.\PhysicalDrive{}", disk.number);
        let handle = open_rw(&path, FILE_FLAG_NO_BUFFERING.0 | FILE_FLAG_WRITE_THROUGH.0)?;
        // Allowed to fail on some drivers; only matters for the last sectors
        let _ = ioctl(&handle, FSCTL_ALLOW_EXTENDED_DASD_IO);
        ioctl(&handle, IOCTL_DISK_DELETE_DRIVE_LAYOUT).map_err(win(format!(
            "clear the partition table of {}",
            disk.label()
        )))?;

        Ok(Self {
            handle,
            size: disk.size,
            sector_size: disk.sector_size,
            _volumes: locked,
            buffer: AlignedBuffer::new(BLOCK),
        })
    }

    /// Lets Windows read the new partition table. The locks are released when
    /// this value is dropped right after.
    pub fn finish(self) -> Result<()> {
        ioctl(&self.handle, IOCTL_DISK_UPDATE_PROPERTIES)
            .map_err(win("let Windows read the new partition table"))
    }
}

impl BlockDevice for PhysicalDrive {
    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        check_bounds(self.size, offset, data.len())?;
        check_aligned(offset, data.len(), self.sector_size)?;
        let mut done = 0;
        while done < data.len() {
            let chunk = (data.len() - done).min(self.buffer.len);
            self.buffer.as_mut()[..chunk].copy_from_slice(&data[done..done + chunk]);
            let mut overlapped = at_offset(offset + done as u64);
            let mut written = 0u32;
            // SAFETY: the aligned buffer holds chunk bytes, the handle is open
            // synchronously, so the OVERLAPPED only carries the offset
            unsafe {
                WriteFile(
                    self.handle.0,
                    Some(&self.buffer.as_ref()[..chunk]),
                    Some(&raw mut written),
                    Some(&raw mut overlapped),
                )
            }
            .map_err(io::Error::from)?;
            if written as usize != chunk {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "short write"));
            }
            done += chunk;
        }
        Ok(())
    }

    fn read_at(&mut self, offset: u64, data: &mut [u8]) -> io::Result<()> {
        check_bounds(self.size, offset, data.len())?;
        check_aligned(offset, data.len(), self.sector_size)?;
        let mut done = 0;
        while done < data.len() {
            let chunk = (data.len() - done).min(self.buffer.len);
            let mut overlapped = at_offset(offset + done as u64);
            let mut read = 0u32;
            // SAFETY: as in write_at
            unsafe {
                ReadFile(
                    self.handle.0,
                    Some(&mut self.buffer.as_mut()[..chunk]),
                    Some(&raw mut read),
                    Some(&raw mut overlapped),
                )
            }
            .map_err(io::Error::from)?;
            if read as usize != chunk {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            data[done..done + chunk].copy_from_slice(&self.buffer.as_ref()[..chunk]);
            done += chunk;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        // SAFETY: the handle is open
        unsafe { FlushFileBuffers(self.handle.0) }.map_err(io::Error::from)
    }
}

fn lock_volume(path: &str) -> Result<Handle> {
    let handle = open_rw(path, 0)?;
    let mut last = None;
    for _ in 0..LOCK_ATTEMPTS {
        match ioctl(&handle, FSCTL_LOCK_VOLUME) {
            Ok(()) => {
                ioctl(&handle, FSCTL_DISMOUNT_VOLUME).map_err(win(format!("dismount {path}")))?;
                return Ok(handle);
            }
            Err(e) => last = Some(e),
        }
        std::thread::sleep(LOCK_WAIT);
    }
    Err(Error::Refused(format!(
        "another program is using the stick ({path}: {}). Close Explorer windows and programs that show it, then try again",
        last.map_or_default(|e| e.message())
    )))
}

fn open_rw(path: &str, flags: u32) -> Result<Handle> {
    let wide = Wide::new(path);
    // SAFETY: the path is a valid null-terminated string
    let handle = unsafe {
        CreateFileW(
            wide.pcwstr(),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(flags),
            None,
        )
    }
    .map_err(win(format!(
        "open {path} for writing (run as administrator?)"
    )))?;
    Ok(Handle(handle))
}

/// A control code without input or output
fn ioctl(handle: &Handle, code: u32) -> ::windows::core::Result<()> {
    let mut returned = 0u32;
    // SAFETY: no buffers are passed
    unsafe {
        DeviceIoControl(
            handle.0,
            code,
            None,
            0,
            None,
            0,
            Some(&raw mut returned),
            None,
        )
    }
}

fn at_offset(offset: u64) -> OVERLAPPED {
    let mut overlapped = OVERLAPPED::default();
    overlapped.Anonymous.Anonymous.Offset = offset as u32;
    overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
    overlapped
}

fn check_aligned(offset: u64, len: usize, sector: u32) -> io::Result<()> {
    let sector = u64::from(sector);
    if !offset.is_multiple_of(sector) || !(len as u64).is_multiple_of(sector) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{len} bytes at {offset} are not aligned to {sector}-byte sectors"),
        ));
    }
    Ok(())
}

/// Memory aligned for unbuffered I/O
struct AlignedBuffer {
    ptr: *mut u8,
    len: usize,
}

impl AlignedBuffer {
    fn new(len: usize) -> Self {
        let layout = Layout::from_size_align(len, ALIGN).expect("valid layout");
        // SAFETY: the layout has a non-zero size
        let ptr = unsafe { alloc_zeroed(layout) };
        assert!(!ptr.is_null(), "out of memory");
        Self { ptr, len }
    }

    fn as_ref(&self) -> &[u8] {
        // SAFETY: ptr is valid for len bytes for the lifetime of self
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    fn as_mut(&mut self) -> &mut [u8] {
        // SAFETY: ptr is valid for len bytes, borrowed mutably once
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.len, ALIGN).expect("valid layout");
        // SAFETY: allocated in new with the same layout
        unsafe { dealloc(self.ptr, layout) };
    }
}

// The buffer is owned and only used through &mut self
unsafe impl Send for AlignedBuffer {}
