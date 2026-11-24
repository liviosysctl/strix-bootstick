//! Finding the disks: `\\.\PhysicalDrive0` and up, their bus, size and
//! name, which volumes and drive letters they carry, and whether Windows runs
//! from them.

use std::ffi::c_void;
use std::mem::size_of;

use ::windows::Win32::Foundation::HANDLE;
use ::windows::Win32::Storage::FileSystem::{
    BusTypeSd, BusTypeUsb, CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
    GetVolumePathNamesForVolumeNameW, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
};
use ::windows::Win32::System::IO::DeviceIoControl;
use ::windows::Win32::System::Ioctl::{
    DISK_GEOMETRY_EX, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_STORAGE_QUERY_PROPERTY,
    PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
    StorageDeviceProperty, VOLUME_DISK_EXTENTS,
};
use ::windows::Win32::System::SystemInformation::GetWindowsDirectoryW;

use super::{Handle, Wide, win};
use crate::Result;
use crate::disks::{Bus, Disk};

/// Physical drive numbers to look at. Windows numbers them without gaps in
/// practice, but a removed stick can leave one, so do not stop at the first.
const MAX_DRIVES: u32 = 64;

/// A volume (`\\?\Volume{...}\`) with the disks it lies on and its letters
pub(crate) struct Volume {
    pub name: String,
    pub disks: Vec<u32>,
    pub letters: Vec<String>,
}

/// All disks Windows sees, with what the app needs to decide about them
pub fn list() -> Result<Vec<Disk>> {
    let volumes = volumes()?;
    let system = system_disks(&volumes);
    let mut disks = Vec::new();
    for number in 0..MAX_DRIVES {
        let Ok(handle) = open_for_query(&format!(r"\\.\PhysicalDrive{number}")) else {
            continue;
        };
        let Ok((bus, removable, vendor, product)) = describe(&handle) else {
            continue;
        };
        let Ok((size, sector_size)) = geometry(&handle) else {
            continue;
        };
        let letters = volumes
            .iter()
            .filter(|v| v.disks.contains(&number))
            .flat_map(|v| v.letters.iter().cloned())
            .collect();
        disks.push(Disk {
            number,
            size,
            sector_size,
            bus,
            removable,
            vendor,
            product,
            system: system.contains(&number),
            letters,
        });
    }
    Ok(disks)
}

/// The disks that hold the Windows folder. Only those are refused outright;
/// a stick that is not USB is refused by its bus anyway.
fn system_disks(volumes: &[Volume]) -> Vec<u32> {
    let mut buffer = [0u16; 260];
    // SAFETY: the buffer is valid for its length
    let len = unsafe { GetWindowsDirectoryW(Some(&mut buffer)) } as usize;
    let windows_dir = String::from_utf16_lossy(&buffer[..len.min(buffer.len())]);
    let drive = windows_dir.get(..2).unwrap_or("C:").to_uppercase();
    volumes
        .iter()
        .filter(|v| v.letters.iter().any(|l| l.eq_ignore_ascii_case(&drive)))
        .flat_map(|v| v.disks.iter().copied())
        .collect()
}

pub(crate) fn volumes() -> Result<Vec<Volume>> {
    let mut name = [0u16; 260];
    // SAFETY: the buffer is valid for its length
    let find = unsafe { FindFirstVolumeW(&mut name) }.map_err(win("list the volumes"))?;
    let mut volumes = Vec::new();
    loop {
        let volume = from_wide(&name);
        let disks = volume_disks(&volume).unwrap_or_default();
        let letters = volume_letters(&volume);
        volumes.push(Volume {
            name: volume,
            disks,
            letters,
        });
        // SAFETY: find is a valid search handle, the buffer is valid
        if unsafe { FindNextVolumeW(find, &mut name) }.is_err() {
            break;
        }
    }
    // SAFETY: find is a valid search handle, closed once
    unsafe {
        let _ = FindVolumeClose(find);
    }
    Ok(volumes)
}

/// The physical drives a volume lies on (more than one for spanned volumes)
fn volume_disks(volume: &str) -> Result<Vec<u32>> {
    // The device itself, without the trailing backslash of the volume name
    let handle = open_for_query(volume.trim_end_matches('\\'))?;
    // u64 elements, so the structure read from it is aligned
    let mut buffer = vec![0u64; 512];
    let mut returned = 0u32;
    // SAFETY: the output buffer is valid and large enough for 200 extents
    unsafe {
        DeviceIoControl(
            handle.0,
            IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
            None,
            0,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            (buffer.len() * 8) as u32,
            Some(&raw mut returned),
            None,
        )
    }
    .map_err(win(format!("disk extents of {volume}")))?;
    // SAFETY: the call filled the buffer with a VOLUME_DISK_EXTENTS
    let extents = unsafe { &*buffer.as_ptr().cast::<VOLUME_DISK_EXTENTS>() };
    let count = extents.NumberOfDiskExtents as usize;
    // SAFETY: the extents follow each other in the buffer, count of them
    let all = unsafe { std::slice::from_raw_parts(extents.Extents.as_ptr(), count) };
    Ok(all.iter().map(|e| e.DiskNumber).collect())
}

fn volume_letters(volume: &str) -> Vec<String> {
    let name = Wide::new(volume);
    let mut buffer = vec![0u16; 1024];
    let mut len = 0u32;
    // SAFETY: the buffer is valid for its length
    if unsafe { GetVolumePathNamesForVolumeNameW(name.pcwstr(), Some(&mut buffer), &raw mut len) }
        .is_err()
    {
        return Vec::new();
    }
    // A list of zero-terminated paths like "E:\", ending with an empty one
    buffer[..len as usize]
        .split(|&c| c == 0)
        .map(String::from_utf16_lossy)
        .filter(|p| p.len() == 3 && p.ends_with(":\\"))
        .map(|p| p[..2].to_uppercase())
        .collect()
}

fn describe(handle: &Handle) -> Result<(Bus, bool, String, String)> {
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        ..Default::default()
    };
    // u64 elements, so the structure read from it is aligned
    let mut words = vec![0u64; 128];
    let mut returned = 0u32;
    // SAFETY: input and output buffers are valid for the given sizes
    unsafe {
        DeviceIoControl(
            handle.0,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some((&raw const query).cast::<c_void>()),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(words.as_mut_ptr().cast::<c_void>()),
            (words.len() * 8) as u32,
            Some(&raw mut returned),
            None,
        )
    }
    .map_err(win("describe the disk"))?;
    // SAFETY: the call filled the buffer with a STORAGE_DEVICE_DESCRIPTOR
    let descriptor = unsafe { &*words.as_ptr().cast::<STORAGE_DEVICE_DESCRIPTOR>() };
    let buffer: Vec<u8> = words.iter().flat_map(|w| w.to_ne_bytes()).collect();
    let bus = match descriptor.BusType {
        t if t == BusTypeUsb => Bus::Usb,
        t if t == BusTypeSd => Bus::Sd,
        #[cfg(feature = "virtual-disks")]
        t if t == ::windows::Win32::Storage::FileSystem::BusTypeFileBackedVirtual => Bus::Usb,
        t => Bus::Other(format!("{}", t.0)),
    };
    let text_at = |offset: u32| -> String {
        let start = offset as usize;
        if start == 0 || start >= buffer.len() {
            return String::new();
        }
        let end = buffer[start..]
            .iter()
            .position(|&b| b == 0)
            .map_or(buffer.len(), |p| start + p);
        String::from_utf8_lossy(&buffer[start..end])
            .trim()
            .to_owned()
    };
    Ok((
        bus,
        descriptor.RemovableMedia,
        text_at(descriptor.VendorIdOffset),
        text_at(descriptor.ProductIdOffset),
    ))
}

fn geometry(handle: &Handle) -> Result<(u64, u32)> {
    let mut geometry = DISK_GEOMETRY_EX::default();
    let mut returned = 0u32;
    // SAFETY: the output buffer is a DISK_GEOMETRY_EX
    unsafe {
        DeviceIoControl(
            handle.0,
            IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
            None,
            0,
            Some((&raw mut geometry).cast::<c_void>()),
            size_of::<DISK_GEOMETRY_EX>() as u32,
            Some(&raw mut returned),
            None,
        )
    }
    .map_err(win("disk geometry"))?;
    Ok((
        u64::try_from(geometry.DiskSize).unwrap_or(0),
        geometry.Geometry.BytesPerSector,
    ))
}

/// Opens a device only to ask it things; needs no administrator rights
pub(crate) fn open_for_query(path: &str) -> Result<Handle> {
    let wide = Wide::new(path);
    // SAFETY: the path is a valid null-terminated string
    let handle: HANDLE = unsafe {
        CreateFileW(
            wide.pcwstr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .map_err(win(format!("open {path}")))?;
    Ok(Handle(handle))
}

fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}
