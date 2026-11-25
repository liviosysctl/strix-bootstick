//! The real thing, on Windows: the core against the disks and the firmware
//! of this PC.

use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, PoisonError};

use bootstick_core::manifest::Manifest;
use bootstick_core::progress::{Observer, Progress, Stage};
use bootstick_core::windows::{Flash, NoBootEntry, disks, flash, system};
use bootstick_core::{Error, download, release};

use crate::flow::Machine;
use crate::wire::{Doing, Failure, Outcome, Release, Stick};

#[derive(Default)]
pub struct Real {
    /// The manifest `release` fetched, what `write` writes
    manifest: Mutex<Option<Manifest>>,
}

impl Machine for Real {
    fn release(&self) -> Result<Release, Failure> {
        let manifest =
            Manifest::fetch(&release::agent()).map_err(|e| Failure::new(&e, Doing::Manifest))?;
        let found = Release::from(&manifest);
        *self.manifest.lock().unwrap_or_else(PoisonError::into_inner) = Some(manifest);
        Ok(found)
    }

    fn sticks(&self, image_len: u64) -> Result<Vec<Stick>, Failure> {
        let disks = disks::list().map_err(|e| Failure::new(&e, Doing::Sticks))?;
        Ok(disks
            .iter()
            .filter_map(|disk| Stick::from_disk(disk, image_len))
            .collect())
    }

    fn write(&self, disk: u32, cancel: &AtomicBool, report: &mut dyn FnMut(&Progress)) -> Outcome {
        let Some(manifest) = self
            .manifest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        else {
            let error = Error::Refused("no release is loaded yet".to_owned());
            return Outcome::Failed(Failure::new(&error, Doing::Write));
        };
        // A sleeping PC in the middle of writing leaves a broken stick
        let _awake = system::keep_awake();
        let mut observer = Forward {
            report,
            cancel,
            stage: Stage::Download,
        };
        let result = (|| {
            let dir = download::cache_dir(&manifest);
            let image =
                download::download_release(&release::agent(), &manifest, &dir, &mut observer)?;
            download::prune_cache(&manifest);
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
                &mut observer,
            )
        })();
        let during = Doing::from(observer.stage);
        match result {
            Ok(report) => Outcome::Done {
                no_boot_next: report.no_boot_entry.map(|why| match why {
                    NoBootEntry::NotUefi => "not-uefi",
                    NoBootEntry::SectorSize(_) => "sector-size",
                    NoBootEntry::Firmware(_) => "firmware",
                }),
            },
            Err(Error::Cancelled) => Outcome::Cancelled { during },
            Err(error) => Outcome::Failed(Failure::new(&error, during)),
        }
    }

    fn restart(&self) -> Result<(), Failure> {
        system::restart().map_err(|e| Failure::new(&e, Doing::Restart))
    }
}

/// Hands the progress of the core on and remembers the stage, for the
/// report of a failure
struct Forward<'a> {
    report: &'a mut dyn FnMut(&Progress),
    cancel: &'a AtomicBool,
    stage: Stage,
}

impl Observer for Forward<'_> {
    fn progress(&mut self, progress: &Progress) {
        self.stage = progress.stage;
        (self.report)(progress);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }
}
