//! Progress feedback on stderr (§3.3).
//!
//! Wall clock appears here and only here: sampling cadence and the `elapsed`
//! field are presentation, never a business input (ADR-015). Because the
//! reporter only ever *reads* atomics written by the analysis thread, switching
//! it off with `-q` cannot change a single byte of the JSONL stream.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use packetsage_core::ProgressCounters;

use crate::color::stderr_is_tty;

/// Cadence of the terminal refresh.
const TTY_INTERVAL: Duration = Duration::from_millis(200);
/// Cadence of the degraded (non-terminal) form.
const PIPE_INTERVAL: Duration = Duration::from_secs(5);
/// Forced refresh every this many packets, whichever comes first.
const PACKET_STRIDE: u64 = 100_000;
/// Sampling granularity of the watchdog thread.
const SAMPLE: Duration = Duration::from_millis(50);

/// Owns the sampling thread for one command.
pub struct Progress {
    done: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    started: Instant,
    interactive: bool,
}

impl Progress {
    /// Starts the reporter unless it is disabled or the thread cannot spawn.
    #[must_use]
    pub fn start(counters: Arc<ProgressCounters>, label: &'static str, quiet: bool) -> Self {
        let started = Instant::now();
        let interactive = stderr_is_tty();
        let done = Arc::new(AtomicBool::new(false));
        let handle = if quiet {
            None
        } else {
            spawn(counters, label, interactive, Arc::clone(&done), started)
        };
        Self {
            done,
            handle,
            started,
            interactive,
        }
    }

    /// Stops the thread and prints the closing line (`done: N packets in T s`).
    pub fn finish(mut self, packets: u64) {
        self.done.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
            if self.interactive {
                // Clear the in-place line before the summary lands on stderr.
                eprint!("\r\x1b[2K");
            }
            eprintln!(
                "done: {packets} packets in {} s",
                seconds(self.started.elapsed())
            );
        }
    }
}

/// The sampling loop's packet counters are integers: the only division is the
/// "every 100k packets" stride, which is exact by construction.
#[allow(clippy::integer_division)]
fn spawn(
    counters: Arc<ProgressCounters>,
    label: &'static str,
    interactive: bool,
    done: Arc<AtomicBool>,
    started: Instant,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("packetsage-progress".to_owned())
        .spawn(move || {
            let interval = if interactive {
                TTY_INTERVAL
            } else {
                PIPE_INTERVAL
            };
            let mut last_line = Instant::now();
            let mut last_stride = 0u64;
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(SAMPLE);
                let snapshot = counters.snapshot();
                let stride_due = snapshot.packets / PACKET_STRIDE > last_stride;
                if last_line.elapsed() < interval && !stride_due {
                    continue;
                }
                last_line = Instant::now();
                last_stride = snapshot.packets / PACKET_STRIDE;
                let line = render(label, snapshot, started.elapsed());
                if interactive {
                    eprint!("\r\x1b[2K{line}");
                } else {
                    eprintln!("{line}");
                }
            }
        })
        .ok()
}

fn render(label: &str, snapshot: packetsage_core::ProgressSnapshot, elapsed: Duration) -> String {
    let seconds = elapsed.as_secs_f64();
    let speed = if seconds > 0.0 {
        snapshot.packets as f64 / seconds
    } else {
        0.0
    };
    format!(
        "{label}: phase={} packets={} bytes={} speed={} elapsed={}",
        snapshot.phase.as_str(),
        snapshot.packets,
        human_bytes(snapshot.bytes),
        human_speed(speed),
        seconds_fmt(seconds)
    )
}

/// `84.3MiB` / `512KiB` / `900B`.
#[must_use]
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

/// `41.2k pkt/s` above a thousand packets per second, `812 pkt/s` below.
#[must_use]
pub fn human_speed(speed: f64) -> String {
    if speed >= 1_000.0 {
        format!("{:.1}k pkt/s", speed / 1_000.0)
    } else {
        format!("{speed:.0} pkt/s")
    }
}

/// `2.9s` with one decimal.
#[must_use]
pub fn seconds_fmt(seconds: f64) -> String {
    format!("{seconds:.1}s")
}

fn seconds(duration: Duration) -> String {
    format!("{:.1}", duration.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_units_are_compact() {
        assert_eq!(human_bytes(900), "900B");
        assert_eq!(human_bytes(1024), "1.0KiB");
        assert_eq!(human_bytes(84_300_000), "80.4MiB");
    }

    #[test]
    fn speed_switches_to_thousands() {
        assert_eq!(human_speed(812.4), "812 pkt/s");
        assert_eq!(human_speed(41_200.0), "41.2k pkt/s");
    }

    #[test]
    fn the_line_matches_the_spec_shape() {
        let snapshot = packetsage_core::ProgressSnapshot {
            packets: 120_000,
            bytes: 88_400_000,
            phase: packetsage_core::ProgressPhase::Parse,
        };
        let line = render("analyze", snapshot, Duration::from_millis(2_900));
        assert!(line.starts_with("analyze: phase=parse packets=120000 bytes="));
        assert!(line.ends_with("elapsed=2.9s"), "{line}");
        assert!(line.contains("pkt/s"));
    }

    #[test]
    fn quiet_start_never_prints() {
        let counters = Arc::new(ProgressCounters::new());
        Progress::start(Arc::clone(&counters), "analyze", true).finish(0);
    }
}
