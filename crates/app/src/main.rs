//! The window of the bootstick app, drawn with Slint. Everything it does is
//! in `bootstick-core`; this crate is the window around it.
//!
//! On Windows it does the real thing. Elsewhere it runs on a simulation
//! (`preview.rs`), to look at the window while working on it:
//! `cargo run -p bootstick-app`, with `BOOTSTICK_PREVIEW` set to a scene.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod flow;
mod format;
mod wire;

#[cfg(windows)]
mod machine;
#[cfg(not(windows))]
mod preview;

use std::sync::Arc;

slint::include_modules!();

fn main() {
    #[cfg(windows)]
    {
        // Two apps must never write at the same time
        let Some(_single) = bootstick_core::windows::system::single_instance("strix-bootstick")
        else {
            return;
        };
        if let Err(error) = run(Arc::new(machine::Real::default())) {
            bootstick_core::windows::system::alert(
                "Installer stick",
                &format!("The window could not be opened: {error}"),
            );
        }
    }

    #[cfg(not(windows))]
    if let Err(error) = run(Arc::new(preview::Simulated::from_env())) {
        eprintln!("The window could not be opened: {error}");
        std::process::exit(1);
    }
}

fn run(machine: Arc<dyn flow::Machine>) -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let _poll = flow::connect(&ui, machine);
    ui.run()
}
