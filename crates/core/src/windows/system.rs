//! Small things the app asks of Windows around the writing itself

use std::path::PathBuf;
use std::process::Command;

use ::windows::Win32::System::Power::{ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState};

use ::windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use ::windows::Win32::System::Threading::CreateMutexW;
use ::windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

use super::{Handle, Wide};
use crate::{IoContext, Result};

/// Keeps the PC from going to sleep while it lives. Windows counts the
/// request per thread, so hold it on the thread that does the work.
pub struct Awake(());

/// A sleeping PC in the middle of writing leaves a broken stick
#[must_use]
pub fn keep_awake() -> Awake {
    // SAFETY: only flags, no pointers
    unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
    Awake(())
}

impl Drop for Awake {
    fn drop(&mut self) {
        // SAFETY: only flags, no pointers
        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    }
}

/// Restarts the PC now, the way "Restart" in the start menu does: programs
/// with unsaved work may still ask. A restart keeps `BootNext`.
pub fn restart() -> Result<()> {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let shutdown = root.join("System32").join("shutdown.exe");
    Command::new(&shutdown)
        .args(["/r", "/t", "0"])
        .spawn()
        .map(drop)
        .context(|| format!("start {}", shutdown.display()))
}

/// A plain Windows message box, for the one case where the app has no window
/// to say it in: the window itself could not be opened.
pub fn alert(title: &str, text: &str) {
    let title = Wide::new(title);
    let text = Wide::new(text);
    // SAFETY: both strings are null-terminated and outlive the call
    unsafe { MessageBoxW(None, text.pcwstr(), title.pcwstr(), MB_OK | MB_ICONERROR) };
}

/// Held while the app runs; a second start finds it and ends
pub struct SingleInstance {
    _mutex: Handle,
}

/// `None` when another copy of the program with this name runs already for
/// this user
#[must_use]
pub fn single_instance(name: &str) -> Option<SingleInstance> {
    let name = Wide::new(&format!(r"Local\{name}"));
    // SAFETY: the name is null-terminated and outlives the call
    let handle = unsafe { CreateMutexW(None, true, name.pcwstr()) }.ok()?;
    // SAFETY: reads the error of the call just made on this thread
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return None;
    }
    Some(SingleInstance {
        _mutex: Handle(handle),
    })
}
