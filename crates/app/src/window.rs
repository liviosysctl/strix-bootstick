//! The Tauri side: the commands the page calls and the events it gets back.
//!
//! | Command | What |
//! |---|---|
//! | `release` | Fetch and check `manifest.json`, remember it |
//! | `sticks` | The USB sticks and SD cards that are plugged in |
//! | `start` | Download, write, check, boot entry: on its own thread |
//! | `cancel` | Stop that thread at its next block |
//! | `restart` | Restart Windows now |
//! | `quit` | Close the app |
//!
//! | Event | When |
//! |---|---|
//! | `step` | Progress of the run, about ten times a second |
//! | `finished` | The run ended: done, cancelled or failed |
//! | `close-requested` | Someone tried to close the window during a run |

// Tauri hands commands their state and the app handle by value
#![allow(clippy::needless_pass_by_value)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use bootstick_core::manifest::Manifest;
use bootstick_core::progress::{Observer, Progress, Stage};
use bootstick_core::windows::{Flash, NoBootEntry, disks, flash, system};
use bootstick_core::{Error, download, release as source};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

use crate::wire::{self, Failure, Outcome, Release, Step, Stick};

/// Progress goes to the page at most this often: enough for a smooth bar,
/// far fewer than the blocks the core reports
const REPORT_EVERY: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Session {
    /// The release `release` fetched, what `start` writes
    manifest: Mutex<Option<Manifest>>,
    running: AtomicBool,
    cancel: Arc<AtomicBool>,
}

impl Session {
    fn manifest(&self) -> Option<Manifest> {
        self.manifest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[tauri::command]
async fn release(session: State<'_, Session>) -> Result<Release, Failure> {
    let manifest = tauri::async_runtime::spawn_blocking(|| Manifest::fetch(&source::agent()))
        .await
        .map_err(|error| Failure {
            kind: "io",
            stage: "manifest",
            detail: error.to_string(),
        })?
        .map_err(|error| Failure::new(&error, "manifest"))?;
    let release = Release::from(&manifest);
    *session
        .manifest
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(manifest);
    Ok(release)
}

#[tauri::command(async)]
fn sticks(session: State<'_, Session>) -> Result<Vec<Stick>, Failure> {
    let image_len = session.manifest().map_or(0, |m| m.iso.size);
    let disks = disks::list().map_err(|error| Failure::new(&error, "sticks"))?;
    Ok(disks
        .iter()
        .filter_map(|disk| Stick::from_disk(disk, image_len))
        .collect())
}

#[tauri::command]
fn start(app: AppHandle, session: State<'_, Session>, disk: u32) -> Result<(), Failure> {
    let refused = |why: &str| Failure::new(&Error::Refused(why.to_owned()), "write");
    let manifest = session
        .manifest()
        .ok_or_else(|| refused("no release is loaded yet"))?;
    if session.running.swap(true, Ordering::SeqCst) {
        return Err(refused("a stick is being written already"));
    }
    session.cancel.store(false, Ordering::SeqCst);
    let cancel = Arc::clone(&session.cancel);
    thread::spawn(move || {
        let outcome = write_stick(&app, &manifest, disk, &cancel);
        app.state::<Session>()
            .running
            .store(false, Ordering::SeqCst);
        // Without a window there is nobody left to tell
        let _ = app.emit("finished", outcome);
    });
    Ok(())
}

#[tauri::command]
fn cancel(session: State<'_, Session>) {
    session.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn restart() -> Result<(), Failure> {
    system::restart().map_err(|error| Failure::new(&error, "restart"))
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// The whole run: download (or reuse the download), write, read back, boot
/// entry
fn write_stick(app: &AppHandle, manifest: &Manifest, disk: u32, cancel: &AtomicBool) -> Outcome {
    let _awake = system::keep_awake();
    let mut reporter = Reporter {
        app,
        cancel,
        stage: Stage::Download,
        last: None,
    };
    let result = (|| {
        let dir = download::cache_dir(manifest);
        let image = download::download_release(&source::agent(), manifest, &dir, &mut reporter)?;
        download::prune_cache(manifest);
        let boot_name = format!("{} installer", manifest.name);
        flash(
            &Flash {
                disk,
                image: &image,
                sha256: &manifest.iso.sha256,
                verify: true,
                boot_entry: Some(&boot_name),
                dry_run: false,
            },
            &mut reporter,
        )
    })();
    match result {
        Ok(report) => Outcome::Done {
            boot_next: report.boot_entry.is_some(),
            no_boot_next: report.no_boot_entry.map(|why| match why {
                NoBootEntry::NotUefi => "not-uefi",
                NoBootEntry::SectorSize(_) => "sector-size",
                NoBootEntry::Firmware(_) => "firmware",
            }),
        },
        Err(Error::Cancelled) => Outcome::Cancelled {
            stage: wire::stage_code(reporter.stage),
        },
        Err(error) => Outcome::Failed(Failure::during(&error, reporter.stage)),
    }
}

/// Passes progress on to the page, thinned out, and remembers the stage for
/// the report of a failure
struct Reporter<'a> {
    app: &'a AppHandle,
    cancel: &'a AtomicBool,
    stage: Stage,
    last: Option<Instant>,
}

impl Observer for Reporter<'_> {
    fn progress(&mut self, progress: &Progress) {
        let due = self.last.is_none_or(|at| at.elapsed() >= REPORT_EVERY);
        if !due && progress.stage == self.stage && progress.done < progress.total {
            return;
        }
        self.stage = progress.stage;
        self.last = Some(Instant::now());
        let _ = self.app.emit("step", Step::from(progress));
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        // A second start shows the window that is already there: two apps
        // must never write at the same time
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .manage(Session::default())
        .invoke_handler(tauri::generate_handler![
            release, sticks, start, cancel, restart, quit
        ])
        .on_window_event(|window, event| {
            // Closing in the middle of a run would leave a half-written
            // stick without a word: the page asks first
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.state::<Session>().running.load(Ordering::SeqCst)
            {
                api.prevent_close();
                let _ = window.emit("close-requested", ());
            }
        })
        .run(tauri::generate_context!());
    if let Err(error) = app {
        // Most likely the WebView2 runtime is missing (old Windows 10)
        system::alert(
            "Installer stick",
            &format!(
                "The window could not be opened: {error}\n\nThe app needs the Microsoft Edge WebView2 runtime, which Windows 10 and 11 normally bring along."
            ),
        );
    }
}
