//! Progress reports and cancelling, shared by download and writer

use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Download,
    Write,
    Verify,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    pub done: u64,
    pub total: u64,
    /// The file being downloaded, empty otherwise
    pub file: String,
}

/// Gets a progress report now and then and can stop the work
pub trait Observer {
    fn progress(&mut self, progress: &Progress);

    fn cancelled(&self) -> bool {
        false
    }
}

/// For callers that do not care
pub struct Silent;

impl Observer for Silent {
    fn progress(&mut self, _: &Progress) {}
}

/// An observer from a closure and a cancel flag the UI can set
pub struct Callback<'a, F: FnMut(&Progress)> {
    pub on_progress: F,
    pub cancel: &'a AtomicBool,
}

impl<F: FnMut(&Progress)> Observer for Callback<'_, F> {
    fn progress(&mut self, progress: &Progress) {
        (self.on_progress)(progress);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

pub(crate) fn check_cancel(observer: &dyn Observer) -> Result<()> {
    if observer.cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
