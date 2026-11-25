//! A simulated PC for working on the window on Linux: the same window, with
//! made-up sticks and a run that takes a few seconds. Never part of the
//! Windows app.
//!
//! `BOOTSTICK_PREVIEW` picks the scene: `none` (no stick), `one`, `many`
//! (the default: two sticks, one too small, one too large), `offline` (the
//! release cannot be fetched), `broken` (writing fails), `bios` (the PC
//! cannot be told to start from the stick).

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use bootstick_core::Error;
use bootstick_core::progress::{Progress, Stage};

use crate::flow::Machine;
use crate::wire::{Doing, Failure, Outcome, Problem, Release, Stick};

pub struct Simulated {
    scene: String,
}

impl Simulated {
    pub fn from_env() -> Self {
        Self {
            scene: std::env::var("BOOTSTICK_PREVIEW").unwrap_or_else(|_| "many".to_owned()),
        }
    }
}

const SIZE: u64 = 2_967_011_328;

fn stick(number: u32, name: &str, size: u64, letters: &[&str], problem: Option<Problem>) -> Stick {
    Stick {
        number,
        name: name.to_owned(),
        size,
        letters: letters.iter().map(|l| (*l).to_owned()).collect(),
        problem,
    }
}

impl Machine for Simulated {
    fn release(&self) -> Result<Release, Failure> {
        thread::sleep(Duration::from_millis(700));
        if self.scene == "offline" {
            let error = Error::Refused(
                "network: io: failed to lookup address information: No such host is known. (os error 11001)"
                    .to_owned(),
            );
            let mut failure = Failure::new(&error, Doing::Manifest);
            failure.kind = "network";
            return Err(failure);
        }
        Ok(Release {
            name: "Strix".to_owned(),
            version: "1.2.0".to_owned(),
            size: SIZE,
            secure_boot_off: false,
        })
    }

    fn sticks(&self, _: u64) -> Result<Vec<Stick>, Failure> {
        let sandisk = stick(2, "SanDisk Ultra", 32_017_047_552, &["E:"], None);
        Ok(match self.scene.as_str() {
            "none" => vec![],
            "many" => vec![
                sandisk,
                stick(
                    3,
                    "Kingston DataTraveler 3.0",
                    62_008_590_336,
                    &["F:", "G:"],
                    None,
                ),
                stick(4, "", 2_000_000_000, &[], Some(Problem::TooSmall)),
                stick(
                    5,
                    "WD Elements 25A2",
                    2_000_398_934_016,
                    &["H:"],
                    Some(Problem::TooLarge),
                ),
            ],
            _ => vec![sandisk],
        })
    }

    fn write(&self, _: u32, cancel: &AtomicBool, report: &mut dyn FnMut(&Progress)) -> Outcome {
        for (stage, seconds) in [(Stage::Download, 6), (Stage::Write, 6), (Stage::Verify, 4)] {
            let steps = seconds * 20;
            for step in 0..=steps {
                if cancel.load(Ordering::Relaxed) {
                    return Outcome::Cancelled {
                        during: stage.into(),
                    };
                }
                if self.scene == "broken" && stage == Stage::Write && step == steps / 7 {
                    let error = Error::Refused(
                        "another program is using the stick (\\\\?\\Volume{6f1c2e0a}: Access is denied.). Close Explorer windows and programs that show it, then try again"
                            .to_owned(),
                    );
                    return Outcome::Failed(Failure::new(&error, Doing::Write));
                }
                report(&Progress {
                    stage,
                    done: SIZE * step / steps,
                    total: SIZE,
                    file: String::new(),
                });
                thread::sleep(Duration::from_millis(50));
            }
        }
        Outcome::Done {
            no_boot_next: (self.scene == "bios").then_some("not-uefi"),
        }
    }

    fn restart(&self) -> Result<(), Failure> {
        thread::sleep(Duration::from_secs(3));
        let error = Error::Refused("a simulated PC does not restart".to_owned());
        Err(Failure::new(&error, Doing::Restart))
    }
}
