//! Between the window and the work: the callbacks of the window start the
//! work on threads of its own, and what the work reports comes back into the
//! window on its event loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use bootstick_core::progress::{Progress, Stage};
use slint::{
    CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode,
    VecModel, Weak,
};

use crate::format::{self, Meter};
use crate::wire::{Failure, Outcome, Problem, Release, Stick};
use crate::{AppWindow, Ask, View};

/// The stick list is read this often while it is on screen
const POLL: Duration = Duration::from_secs(2);
/// Progress reaches the window at most this often: enough for a smooth line,
/// far fewer than the blocks the core reports
const REPORT_EVERY: Duration = Duration::from_millis(100);

/// What the window needs done. On Windows the real thing, elsewhere a
/// simulation to look at the window while working on it.
pub trait Machine: Send + Sync + 'static {
    /// Fetches the newest release and remembers it for `write`
    fn release(&self) -> Result<Release, Failure>;
    /// The sticks that are plugged in
    fn sticks(&self, image_len: u64) -> Result<Vec<Stick>, Failure>;
    /// Download, write, check, boot entry. `report` gets every progress
    /// report of the core; `cancel` stops the work at its next block.
    fn write(&self, disk: u32, cancel: &AtomicBool, report: &mut dyn FnMut(&Progress)) -> Outcome;
    /// Restarts the PC now
    fn restart(&self) -> Result<(), Failure>;
}

struct Shared {
    machine: Arc<dyn Machine>,
    /// The release the window shows, once it is known
    release: Mutex<Option<Release>>,
    cancel: Arc<AtomicBool>,
    running: AtomicBool,
    /// A read of the stick list is under way
    polling: AtomicBool,
}

/// Wires the window to the machine and starts looking for the release
pub fn connect(ui: &AppWindow, machine: Arc<dyn Machine>) -> Timer {
    let shared = Arc::new(Shared {
        machine,
        release: Mutex::new(None),
        cancel: Arc::new(AtomicBool::new(false)),
        running: AtomicBool::new(false),
        polling: AtomicBool::new(false),
    });

    ui.set_app_version(env!("CARGO_PKG_VERSION").into());

    {
        let weak = ui.as_weak();
        let shared = shared.clone();
        ui.on_retry(move || {
            let Some(ui) = weak.upgrade() else { return };
            if shared.release().is_some() {
                ui.set_stopped(false);
                ui.set_view(View::Pick);
            } else {
                load(&ui, &shared);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let shared = shared.clone();
        ui.on_refresh_sticks(move || poll(&weak, &shared));
    }
    {
        let weak = ui.as_weak();
        let shared = shared.clone();
        ui.on_go(move |disk| {
            let Some(ui) = weak.upgrade() else { return };
            let Ok(disk) = u32::try_from(disk) else {
                return;
            };
            start(&ui, &shared, disk);
        });
    }
    {
        let shared = shared.clone();
        ui.on_stop(move || shared.cancel.store(true, Ordering::SeqCst));
    }
    {
        let weak = ui.as_weak();
        let shared = shared.clone();
        ui.on_restart(move || {
            let weak = weak.clone();
            let machine = shared.machine.clone();
            thread::spawn(move || {
                if let Err(failure) = machine.restart() {
                    let _ = weak.upgrade_in_event_loop(move |ui| fail(&ui, &failure));
                }
            });
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_describe_stick(move |number| {
            weak.upgrade()
                .and_then(|ui| u32::try_from(number).ok().map(|n| stick_label(&ui, n)))
                .unwrap_or_default()
                .into()
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_next_stick(move |current, step| {
            weak.upgrade()
                .map_or(current, |ui| next_stick(&ui, current, step))
        });
    }
    ui.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    {
        let weak = ui.as_weak();
        ui.on_minimize(move || {
            if let Some(ui) = weak.upgrade() {
                ui.window().set_minimized(true);
            }
        });
    }
    {
        // Closing in the middle of a run would leave a half-written stick
        // without a word: the window asks first
        let weak = ui.as_weak();
        let shared = shared.clone();
        ui.window().on_close_requested(move || {
            if !shared.running.load(Ordering::SeqCst) {
                return CloseRequestResponse::HideWindow;
            }
            if let Some(ui) = weak.upgrade() {
                ui.invoke_ask_stop(true);
            }
            CloseRequestResponse::KeepWindowShown
        });
    }

    load(ui, &shared);

    // The list follows the sticks that are plugged in and pulled out
    let timer = Timer::default();
    let weak = ui.as_weak();
    timer.start(TimerMode::Repeated, POLL, move || poll(&weak, &shared));
    timer
}

impl Shared {
    fn release(&self) -> Option<Release> {
        self.release
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn load(ui: &AppWindow, shared: &Arc<Shared>) {
    ui.set_view(View::Loading);
    let weak = ui.as_weak();
    let shared = shared.clone();
    thread::spawn(move || {
        let result = shared.machine.release();
        if let Ok(found) = &result {
            *shared
                .release
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(found.clone());
        }
        let _ = weak.upgrade_in_event_loop(move |ui| match result {
            Ok(found) => {
                ui.set_name(found.name.into());
                ui.set_version(found.version.into());
                ui.set_size(format::size(found.size).into());
                ui.set_stick_size(format::stick_size(found.size).into());
                ui.set_secure_boot_off(found.secure_boot_off);
                ui.set_stopped(false);
                ui.set_view(View::Pick);
            }
            Err(failure) => fail(&ui, &failure),
        });
    });
}

fn poll(weak: &Weak<AppWindow>, shared: &Arc<Shared>) {
    let Some(ui) = weak.upgrade() else { return };
    let Some(image_len) = shared.release().map(|r| r.size) else {
        return;
    };
    // An open question names a stick: the list waits until it is answered
    if ui.get_view() != View::Pick || ui.get_ask() != Ask::None {
        return;
    }
    if shared.polling.swap(true, Ordering::SeqCst) {
        return;
    }
    let weak = weak.clone();
    let shared = shared.clone();
    thread::spawn(move || {
        let result = shared.machine.sticks(image_len);
        shared.polling.store(false, Ordering::SeqCst);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if ui.get_view() != View::Pick || ui.get_ask() != Ask::None {
                return;
            }
            match result {
                Ok(sticks) => show_sticks(&ui, &sticks),
                Err(failure) => fail(&ui, &failure),
            }
        });
    });
}

fn show_sticks(ui: &AppWindow, sticks: &[Stick]) {
    let usable: Vec<u32> = sticks
        .iter()
        .filter(|s| s.problem.is_none())
        .map(|s| s.number)
        .collect();
    let chosen = u32::try_from(ui.get_chosen()).ok();
    // One usable stick is the one that is meant; with several the user says which
    if !chosen.is_some_and(|c| usable.contains(&c)) {
        ui.set_chosen(match usable.as_slice() {
            [only] => i32::try_from(*only).unwrap_or(-1),
            _ => -1,
        });
    }
    let rows: Vec<crate::Stick> = sticks
        .iter()
        .map(|s| crate::Stick {
            number: i32::try_from(s.number).unwrap_or(-1),
            name: s.name.clone().into(),
            letters: s.letters.join(", ").into(),
            size: format::size(s.size).into(),
            problem: match s.problem {
                None => 0,
                Some(Problem::TooSmall) => 1,
                Some(Problem::TooLarge) => 2,
            },
        })
        .collect();
    // Only a changed list is set again, so the rows do not flicker
    let old = ui.get_sticks();
    let same = old.row_count() == rows.len()
        && rows
            .iter()
            .enumerate()
            .all(|(i, r)| old.row_data(i).as_ref() == Some(r));
    if !same {
        ui.set_usable(i32::try_from(usable.len()).unwrap_or(0));
        ui.set_sticks(ModelRc::new(VecModel::from(rows)));
    }
}

fn start(ui: &AppWindow, shared: &Arc<Shared>, disk: u32) {
    if shared.running.swap(true, Ordering::SeqCst) {
        return;
    }
    let label = stick_label(ui, disk);
    ui.set_stick_label(label.into());
    ui.set_stage(1);
    ui.set_share(0.0);
    ui.set_measuring(true);
    ui.set_rate(SharedString::new());
    ui.set_minutes_left(-1);
    ui.set_stopping(false);
    ui.set_closing(false);
    ui.set_stopped(false);
    ui.set_restarting(false);
    ui.set_view(View::Work);

    shared.cancel.store(false, Ordering::SeqCst);
    let weak = ui.as_weak();
    let shared = shared.clone();
    thread::spawn(move || {
        let mut reporter = Reporter::new(weak.clone());
        let outcome = shared
            .machine
            .write(disk, &shared.cancel, &mut |p| reporter.report(p));
        shared.running.store(false, Ordering::SeqCst);
        let _ = weak.upgrade_in_event_loop(move |ui| finished(&ui, outcome));
    });
}

/// The next stick after `current` that can be chosen, round the list.
/// With none chosen, down starts at the top and up at the bottom.
fn next_stick(ui: &AppWindow, current: i32, step: i32) -> i32 {
    let usable: Vec<i32> = ui
        .get_sticks()
        .iter()
        .filter(|s| s.problem == 0)
        .map(|s| s.number)
        .collect();
    let Some(count) = i32::try_from(usable.len()).ok().filter(|c| *c > 0) else {
        return current;
    };
    let next = match usable.iter().position(|n| *n == current) {
        Some(at) => (i32::try_from(at).unwrap_or(0) + step).rem_euclid(count),
        None if step > 0 => 0,
        None => count - 1,
    };
    usable[usize::try_from(next).unwrap_or(0)]
}

/// For example `SanDisk Ultra, 32 GB (E:)`, for the question before erasing and
/// the done view
fn stick_label(ui: &AppWindow, disk: u32) -> String {
    let sticks = ui.get_sticks();
    let Some(stick) = sticks
        .iter()
        .find(|s| u32::try_from(s.number).ok() == Some(disk))
    else {
        return String::new();
    };
    let letters = if stick.letters.is_empty() {
        String::new()
    } else {
        format!(" ({})", stick.letters)
    };
    format!("{}, {}{letters}", stick.name, stick.size)
}

/// Passes progress on to the window, thinned out, with rate and time left
struct Reporter {
    weak: Weak<AppWindow>,
    stage: Option<Stage>,
    last: Option<Instant>,
    meter: Meter,
}

impl Reporter {
    fn new(weak: Weak<AppWindow>) -> Self {
        Self {
            weak,
            stage: None,
            last: None,
            meter: Meter::new(),
        }
    }

    fn report(&mut self, progress: &Progress) {
        let now = Instant::now();
        let new_stage = self.stage != Some(progress.stage);
        if new_stage {
            self.stage = Some(progress.stage);
            self.meter = Meter::new();
        }
        let measured = self.meter.sample(now, progress.done, progress.total);
        let due = self
            .last
            .is_none_or(|at| now.duration_since(at) >= REPORT_EVERY);
        if !new_stage && !due && progress.done < progress.total {
            return;
        }
        self.last = Some(now);

        let stage = match progress.stage {
            Stage::Download => 1,
            Stage::Write => 2,
            Stage::Verify => 3,
        };
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let share = if progress.total == 0 {
            1.0
        } else {
            (progress.done as f64 / progress.total as f64) as f32
        };
        let done = format::size(progress.done);
        let total = format::size(progress.total);
        let (rate, left) = match measured {
            Some((speed, seconds)) if progress.done < progress.total => {
                (format::rate(speed), format::minutes_left(seconds))
            }
            _ => (String::new(), -1),
        };
        let _ = self.weak.upgrade_in_event_loop(move |ui| {
            ui.set_stage(stage);
            ui.set_share(share);
            ui.set_measuring(false);
            ui.set_amount_done(done.into());
            ui.set_amount_total(total.into());
            ui.set_rate(rate.into());
            ui.set_minutes_left(left);
        });
    }
}

fn finished(ui: &AppWindow, outcome: Outcome) {
    if ui.get_ask() != Ask::About {
        ui.set_ask(Ask::None);
    }
    match outcome {
        Outcome::Done { no_boot_next } => {
            ui.set_boot_next(no_boot_next.is_none());
            ui.set_no_boot_next(no_boot_next.unwrap_or_default().into());
            ui.set_restarting(false);
            ui.set_view(View::Done);
        }
        Outcome::Cancelled { during } => {
            if ui.get_closing() {
                let _ = slint::quit_event_loop();
                return;
            }
            // Stopped before the stick was touched: nothing to say
            ui.set_stopped(during.step() > 1);
            ui.set_stopping(false);
            ui.set_view(View::Pick);
        }
        Outcome::Failed(failure) => fail(ui, &failure),
    }
}

fn fail(ui: &AppWindow, failure: &Failure) {
    ui.set_ask(Ask::None);
    ui.set_fail_kind(failure.kind.into());
    ui.set_fail_stage(failure.doing.code().into());
    ui.set_fail_step(failure.doing.step());
    ui.set_fail_detail(failure.detail.clone().into());
    ui.set_restarting(false);
    ui.set_view(View::Failed);
}
