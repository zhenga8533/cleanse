use std::time::Duration;

use cleanse_model::{Candidate, Size};

use crate::SECONDS_PER_DAY;

/// Bytes in binary units, like `3.4 GiB`.
pub fn bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "one decimal place doesn't need full precision"
    )]
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// A count with thousands separators, like `374,735`.
pub fn count(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// How long ago something happened, in whole days.
pub fn ago(days: u64) -> String {
    match days {
        0 => "today".to_owned(),
        1 => "yesterday".to_owned(),
        days => format!("{days} days ago"),
    }
}

/// When something was last modified, from seconds since the Unix epoch, like
/// `modified 12 days ago`.
pub fn modified(at: u64, now: u64) -> String {
    format!("modified {}", ago(now.saturating_sub(at) / SECONDS_PER_DAY))
}

pub fn seconds(duration: Duration) -> String {
    format!("{:.1} s", duration.as_secs_f64())
}

/// How much space an item takes and frees, like `1.2 GiB` or `1.2 GiB (at least 800 MiB freed)`.
pub fn size(size: &Size) -> String {
    if size.guaranteed < size.on_disk {
        format!(
            "{} (at least {} freed)",
            bytes(size.on_disk),
            bytes(size.guaranteed)
        )
    } else {
        bytes(size.on_disk)
    }
}

pub fn total(candidates: &[&Candidate]) -> Size {
    candidates
        .iter()
        .map(|candidate| candidate.found.size)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_use_binary_units() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(3_650_722_201), "3.4 GiB");
    }

    #[test]
    fn days_read_as_how_long_ago() {
        assert_eq!(ago(0), "today");
        assert_eq!(ago(1), "yesterday");
        assert_eq!(ago(12), "12 days ago");
        let now = 100 * SECONDS_PER_DAY;
        assert_eq!(
            modified(now - 3 * SECONDS_PER_DAY - 5, now),
            "modified 3 days ago"
        );
        // A clock set back, or a file dated in the future.
        assert_eq!(modified(now + 60, now), "modified today");
    }

    #[test]
    fn counts_group_thousands() {
        assert_eq!(count(7), "7");
        assert_eq!(count(1000), "1,000");
        assert_eq!(count(374_735), "374,735");
    }
}
