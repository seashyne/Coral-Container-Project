//! Terminal rendering helpers for pull progress.

use super::{PullPhase, PullProgress};

/// Renders a fixed-width ASCII progress bar, e.g. `[####------] 40%`.
pub fn render_bar(percent: u8, width: usize) -> String {
    let width = width.max(3);
    let filled = ((usize::from(percent.min(100)) * width) + 50) / 100;
    let filled = filled.min(width);
    format!("[{}{}]", "#".repeat(filled), "-".repeat(width - filled))
}

/// Formats a byte count as `12.3 MiB` / `456 B`.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// One-line human-readable rendering of a progress snapshot for terminal output.
pub fn render_line(p: &PullProgress) -> String {
    let bar = render_bar(p.percent, 28);
    let layers = if p.layers_total > 0 {
        format!("{}/{} layers", p.layers_done, p.layers_total)
    } else {
        format!("{} layers", p.layers_done)
    };
    let bytes = if p.bytes_total > 0 {
        format!(
            "{} / {}",
            human_bytes(p.bytes_downloaded),
            human_bytes(p.bytes_total)
        )
    } else {
        human_bytes(p.bytes_downloaded)
    };
    let secs = p.elapsed_ms as f64 / 1000.0;
    let rate = if secs > 0.05 {
        format!("{}/s", human_bytes((p.bytes_downloaded as f64 / secs) as u64))
    } else {
        String::new()
    };
    let mut line = format!(
        "{bar} {:>3}%  {layers}  {bytes}",
        p.percent,
    );
    if !rate.is_empty() {
        line.push_str(&format!("  {rate}"));
    }
    if p.phase == PullPhase::Extracting {
        line = format!("{bar} {:>3}%  {layers}  extracting", p.percent);
    } else if p.phase == PullPhase::Finalizing {
        line = format!("{bar} 100%  {layers}  finalizing");
    }
    line
}
