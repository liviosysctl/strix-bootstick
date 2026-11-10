//! UEFI variables: a boot entry for the stick and `BootNext`, which the
//! firmware uses for exactly one start and then deletes. `BootOrder` stays
//! untouched, so Windows remains the normal start.

use std::ffi::c_void;

use ::windows::Win32::Foundation::{ERROR_ENVVAR_NOT_FOUND, HANDLE, LUID};
use ::windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    SE_SYSTEM_ENVIRONMENT_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use ::windows::Win32::System::SystemInformation::{
    FIRMWARE_TYPE, FirmwareTypeUefi, GetFirmwareType,
};
use ::windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use ::windows::Win32::System::WindowsProgramming::{
    GetFirmwareEnvironmentVariableExW, SetFirmwareEnvironmentVariableExW,
};

use super::{Handle, Wide, win};
use crate::boot_entry::{self, BOOT_FILE};
use crate::partition::EfiPartition;
use crate::{Error, Result};

/// `EFI_GLOBAL_VARIABLE`, the namespace of `Boot####`, `BootOrder` and `BootNext`
const GLOBAL: &str = "{8BE4DF61-93CA-11D2-AA0D-00E098032B8C}";

/// Non-volatile, boot service and runtime access
const ATTRIBUTES: u32 = 0x7;

/// Numbers probed for existing entries. Firmware and Windows use low numbers;
/// entries outside this range are not overwritten either, because `BootOrder`
/// lists them.
const PROBE: u16 = 0x100;

#[must_use]
pub fn is_uefi() -> bool {
    let mut kind = FIRMWARE_TYPE::default();
    // SAFETY: kind is a valid out pointer
    unsafe { GetFirmwareType(&raw mut kind) }.is_ok() && kind == FirmwareTypeUefi
}

/// Creates or updates the boot entry with this description for the stick's
/// EFI partition and sets `BootNext` to it. Returns the entry number.
pub fn boot_next_once(description: &str, esp: &EfiPartition) -> Result<u16> {
    enable_privilege()?;

    let mut existing: Vec<(u16, Option<String>)> = Vec::new();
    for number in 0..PROBE {
        if let Some(data) = read(&boot_entry::variable_name(number))? {
            existing.push((number, boot_entry::description(&data)));
        }
    }
    if let Some(order) = read("BootOrder")? {
        for n in order
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
        {
            if !existing.iter().any(|(e, _)| *e == n) {
                existing.push((n, None));
            }
        }
    }
    let number = boot_entry::pick_number(&existing, description)
        .ok_or_else(|| Error::Refused("no free boot entry number".to_owned()))?;

    let option = boot_entry::load_option(description, esp, BOOT_FILE);
    write(&boot_entry::variable_name(number), &option)?;
    write("BootNext", &number.to_le_bytes())?;
    Ok(number)
}

fn read(name: &str) -> Result<Option<Vec<u8>>> {
    let wide_name = Wide::new(name);
    let guid = Wide::new(GLOBAL);
    let mut buffer = vec![0u8; 4096];
    let mut attributes = 0u32;
    // SAFETY: the buffer is valid for its length, the strings are terminated
    let len = unsafe {
        GetFirmwareEnvironmentVariableExW(
            wide_name.pcwstr(),
            guid.pcwstr(),
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            buffer.len() as u32,
            Some(&raw mut attributes),
        )
    };
    if len == 0 {
        let error = ::windows::core::Error::from_thread();
        if error.code() == ERROR_ENVVAR_NOT_FOUND.to_hresult() {
            return Ok(None);
        }
        return Err(Error::Windows {
            context: format!("read the firmware variable {name}"),
            source: error,
        });
    }
    buffer.truncate(len as usize);
    Ok(Some(buffer))
}

fn write(name: &str, data: &[u8]) -> Result<()> {
    let wide_name = Wide::new(name);
    let guid = Wide::new(GLOBAL);
    // SAFETY: data is valid for its length, the strings are terminated
    unsafe {
        SetFirmwareEnvironmentVariableExW(
            wide_name.pcwstr(),
            guid.pcwstr(),
            Some(data.as_ptr().cast::<c_void>()),
            data.len() as u32,
            ATTRIBUTES,
        )
    }
    .map_err(win(format!("write the firmware variable {name}")))
}

/// Firmware variables need `SeSystemEnvironmentPrivilege`. Administrators have
/// it, but it is off until a program turns it on for itself.
fn enable_privilege() -> Result<()> {
    let mut token = HANDLE::default();
    // SAFETY: token is a valid out pointer
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &raw mut token,
        )
    }
    .map_err(win("open the process token"))?;
    let token = Handle(token);

    let mut luid = LUID::default();
    // SAFETY: the name is a constant string, luid a valid out pointer
    unsafe { LookupPrivilegeValueW(None, SE_SYSTEM_ENVIRONMENT_NAME, &raw mut luid) }
        .map_err(win("look up the firmware privilege"))?;
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    // SAFETY: the token is open with TOKEN_ADJUST_PRIVILEGES
    unsafe { AdjustTokenPrivileges(token.0, false, Some(&raw const privileges), 0, None, None) }
        .map_err(win("enable the firmware privilege (run as administrator?)"))?;
    // AdjustTokenPrivileges succeeds even when the privilege is missing
    let error = ::windows::core::Error::from_thread();
    if error.code() == ::windows::Win32::Foundation::ERROR_NOT_ALL_ASSIGNED.to_hresult() {
        return Err(Error::Refused(
            "the firmware boot entry needs administrator rights".to_owned(),
        ));
    }
    Ok(())
}
