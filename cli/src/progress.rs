use quiczilla_core::types::ProgressInfo;
use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

const RENDER_INTERVAL: Duration = Duration::from_millis(500);
const BAR_WIDTH: usize = 28;

pub struct ProgressRenderer {
    enabled: bool,
    quiet: bool,
    has_live_line: bool,
    last_rendered: Option<Instant>,
}

impl ProgressRenderer {
    pub fn new(quiet: bool, no_progress: bool) -> Self {
        Self {
            enabled: !quiet && !no_progress && io::stderr().is_terminal(),
            quiet,
            has_live_line: false,
            last_rendered: None,
        }
    }

    pub fn status(&mut self, message: &str) {
        if self.quiet {
            return;
        }

        self.clear_live_line();
        let _ = writeln!(io::stderr(), "{message}");
    }

    pub fn progress(&mut self, file_name: &str, progress: &ProgressInfo) {
        if !self.enabled {
            return;
        }

        let now = Instant::now();
        if self
            .last_rendered
            .is_some_and(|last| now.duration_since(last) < RENDER_INTERVAL)
            && !progress.is_completed
        {
            return;
        }

        self.last_rendered = Some(now);
        self.has_live_line = true;
        let _ = write!(
            io::stderr(),
            "\r\x1b[2K{}",
            render_progress_line(file_name, progress)
        );
        let _ = io::stderr().flush();
    }

    pub fn completed(
        &mut self,
        file_name: &str,
        bytes: u64,
        duration: Duration,
        checksum_verified: bool,
    ) {
        if self.quiet {
            return;
        }

        self.clear_live_line();
        let seconds = duration.as_secs_f64().max(0.001);
        let average_rate = bytes as f64 / seconds;
        let verified = if checksum_verified {
            "  ·  verified"
        } else {
            ""
        };
        let _ = writeln!(
            io::stderr(),
            "{file_name}  {} transferred in {}  ·  {}/s{verified}",
            format_bytes(bytes),
            format_duration(duration),
            format_bytes(average_rate as u64),
        );
    }

    pub fn paused(&mut self, file_name: &str, progress: Option<&ProgressInfo>) {
        if !self.enabled {
            return;
        }

        self.has_live_line = true;
        let line = if let Some(prog) = progress {
            render_paused_line(file_name, prog)
        } else {
            format!("{file_name}  [PAUSED - press Space to resume]")
        };
        let _ = write!(io::stderr(), "\r\x1b[2K{}", line);
        let _ = io::stderr().flush();
    }

    pub fn resume(&mut self) {
        self.last_rendered = None;
    }

    pub fn failed(&mut self, reason: &str) {
        self.clear_live_line();
        let _ = writeln!(io::stderr(), "Transfer failed: {reason}");
    }

    fn clear_live_line(&mut self) {
        if self.has_live_line {
            let _ = write!(io::stderr(), "\r\x1b[2K");
            let _ = io::stderr().flush();
            self.has_live_line = false;
        }
    }
}

fn render_progress_line(file_name: &str, progress: &ProgressInfo) -> String {
    let percentage = progress.percentage.clamp(0.0, 100.0);
    let filled = ((percentage / 100.0) * BAR_WIDTH as f64).round() as usize;
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled));
    let eta = progress
        .estimated_remaining_secs
        .map(|seconds| {
            format!(
                "  ETA {}",
                format_duration(Duration::from_secs_f64(seconds.max(0.0)))
            )
        })
        .unwrap_or_default();

    format!(
        "{file_name}  {:>3.0}% |{bar}| {} / {}  {}/s{eta}",
        percentage,
        format_bytes(progress.bytes_transferred),
        format_bytes(progress.total_bytes),
        format_bytes(progress.speed_bytes_per_second.max(0.0) as u64),
    )
}

fn render_paused_line(file_name: &str, progress: &ProgressInfo) -> String {
    let percentage = progress.percentage.clamp(0.0, 100.0);
    let filled = ((percentage / 100.0) * BAR_WIDTH as f64).round() as usize;
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled));

    format!(
        "{file_name}  {:>3.0}% |{bar}| {} / {}  [PAUSED - press Space to resume]",
        percentage,
        format_bytes(progress.bytes_transferred),
        format_bytes(progress.total_bytes),
    )
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60)
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}s", seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_progress_with_rate_and_eta() {
        let line = render_progress_line(
            "archive.tar.zst",
            &ProgressInfo {
                bytes_transferred: 4 * 1024 * 1024,
                total_bytes: 10 * 1024 * 1024,
                speed_bytes_per_second: 2.0 * 1024.0 * 1024.0,
                percentage: 40.0,
                estimated_remaining_secs: Some(3.0),
                is_completed: false,
                average_speed_bytes_per_second: None,
                total_time_secs: None,
            },
        );

        assert!(line.contains("40%"));
        assert!(line.contains("4.00 MiB / 10.00 MiB"));
        assert!(line.contains("2.00 MiB/s"));
        assert!(line.contains("ETA 3s"));
    }

    #[test]
    fn formats_long_duration_compactly() {
        assert_eq!(format_duration(Duration::from_secs(3725)), "1h 02m");
    }

    #[test]
    fn formats_paused_line() {
        let line = render_paused_line(
            "large_file.iso",
            &ProgressInfo {
                bytes_transferred: 5 * 1024 * 1024 * 1024,
                total_bytes: 10 * 1024 * 1024 * 1024,
                speed_bytes_per_second: 0.0,
                percentage: 50.0,
                estimated_remaining_secs: None,
                is_completed: false,
                average_speed_bytes_per_second: None,
                total_time_secs: None,
            },
        );
        assert!(line.contains("50%"));
        assert!(line.contains("5.00 GiB / 10.00 GiB"));
        assert!(line.contains("[PAUSED - press Space to resume]"));
    }
}
