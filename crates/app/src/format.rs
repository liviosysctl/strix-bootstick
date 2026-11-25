//! Numbers as the window shows them. Sizes count like stick packages do
//! (1 GB = 1000^3 bytes); the decimal point is a point in English and in
//! Swiss German alike.

use std::time::{Duration, Instant};

/// Sizes sticks are sold in, in GB
const STICK_SIZES: [u64; 9] = [1, 2, 4, 8, 16, 32, 64, 128, 256];

/// "3.0 GB", "32 GB", "2.0 TB"
#[must_use]
pub fn size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let gb = bytes as f64 / 1e9;
    if gb >= 1000.0 {
        format!("{:.1} TB", gb / 1000.0)
    } else if gb >= 10.0 {
        format!("{gb:.0} GB")
    } else {
        format!("{gb:.1} GB")
    }
}

/// The smallest usual stick that holds `bytes`: "at least 4 GB" reads better
/// than "at least 2.97 GB"
#[must_use]
pub fn stick_size(bytes: u64) -> String {
    STICK_SIZES
        .iter()
        .find(|gb| **gb * 1_000_000_000 >= bytes)
        .map_or_else(|| size(bytes), |gb| format!("{gb} GB"))
}

/// "197 MB", "4.5 MB": per second, the window adds the "/s"
#[must_use]
pub fn rate(bytes_per_second: f64) -> String {
    let mb = bytes_per_second / 1e6;
    if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

/// Time left for the window: 0 for less than a minute, otherwise minutes
#[must_use]
pub fn minutes_left(seconds: f64) -> i32 {
    if seconds < 45.0 {
        0
    } else if seconds < 90.0 {
        1
    } else {
        #[allow(clippy::cast_possible_truncation)]
        let minutes = (seconds / 60.0).round() as i32;
        minutes
    }
}

/// Rate and time left over the last few seconds of one stage
pub struct Meter {
    samples: Vec<(Instant, u64)>,
}

/// Rate and time left are measured over this window
const WINDOW: Duration = Duration::from_secs(8);
/// and shown once it holds this much
const MIN_SPAN: Duration = Duration::from_secs(2);

impl Meter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }

    /// Takes a report and gives bytes per second and seconds left, once
    /// enough time has passed to say
    pub fn sample(&mut self, at: Instant, done: u64, total: u64) -> Option<(f64, f64)> {
        self.samples.push((at, done));
        while self.samples.len() > 2 && at.duration_since(self.samples[0].0) > WINDOW {
            self.samples.remove(0);
        }
        let (first_at, first_done) = self.samples[0];
        let span = at.duration_since(first_at);
        if span < MIN_SPAN || done <= first_done {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        let speed = (done - first_done) as f64 / span.as_secs_f64();
        #[allow(clippy::cast_precision_loss)]
        let left = total.saturating_sub(done) as f64 / speed;
        Some((speed, left))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(2_967_011_328), "3.0 GB");
        assert_eq!(size(32_017_047_552), "32 GB");
        assert_eq!(size(2_000_398_934_016), "2.0 TB");
        assert_eq!(stick_size(2_967_011_328), "4 GB");
        assert_eq!(stick_size(500_000_000), "1 GB");
        assert_eq!(stick_size(300_000_000_000), "300 GB");
        assert_eq!(rate(197_000_000.0), "197 MB");
        assert_eq!(rate(4_500_000.0), "4.5 MB");
    }

    #[test]
    fn time_left() {
        assert_eq!(minutes_left(10.0), 0);
        assert_eq!(minutes_left(60.0), 1);
        assert_eq!(minutes_left(150.0), 3);
    }

    #[test]
    fn meter_waits_for_two_seconds() {
        let start = Instant::now();
        let mut meter = Meter::new();
        assert_eq!(meter.sample(start, 0, 1000), None);
        assert_eq!(
            meter.sample(start + Duration::from_secs(1), 100, 1000),
            None
        );
        let (speed, left) = meter
            .sample(start + Duration::from_secs(2), 200, 1000)
            .unwrap();
        assert!((speed - 100.0).abs() < 1e-9);
        assert!((left - 8.0).abs() < 1e-9);
    }
}
