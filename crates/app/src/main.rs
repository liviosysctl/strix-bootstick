//! The window of the bootstick app. Everything it does is in `bootstick-core`;
//! this crate is the Tauri shell around it: commands the page calls, progress
//! events back to the page, and the page itself (`ui/`, built with the Strix design system).
//!
//! The window only exists on Windows. Elsewhere the crate compiles to a stub,
//! so the workspace and its tests build on any system.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(any(windows, test))]
mod wire;

#[cfg(windows)]
mod window;

fn main() {
    #[cfg(windows)]
    window::run();

    #[cfg(not(windows))]
    {
        eprintln!("The bootstick app only runs on Windows.");
        std::process::exit(1);
    }
}
