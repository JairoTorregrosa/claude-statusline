//! Human labels for rate-limit reset epochs: "14:30" when the reset lands
//! today (local time), a weekday abbreviation ("Thu") otherwise.

use chrono::{DateTime, Local};

/// Epochs after 2100-01-01 are assumed to be milliseconds — a defensive
/// guard against the classic seconds-vs-ms API drift.
const MS_EPOCH_THRESHOLD: i64 = 4_102_444_800;

pub fn reset_label(epoch: i64) -> Option<String> {
    let epoch = if epoch > MS_EPOCH_THRESHOLD {
        epoch / 1000
    } else {
        epoch
    };
    let dt = DateTime::from_timestamp(epoch, 0)?.with_timezone(&Local);
    let now = Local::now();
    if dt.date_naive() == now.date_naive() {
        Some(dt.format("%H:%M").to_string())
    } else {
        Some(dt.format("%a").to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_renders_as_clock_time() {
        let label = reset_label(Local::now().timestamp()).unwrap();
        assert!(label.contains(':'), "expected HH:MM, got {label}");
    }

    #[test]
    fn far_future_renders_as_weekday() {
        let label = reset_label(Local::now().timestamp() + 3 * 86_400).unwrap();
        assert!(!label.contains(':'), "expected weekday, got {label}");
        assert!((2..=3).contains(&label.chars().count()));
    }

    #[test]
    fn invalid_epoch_is_none() {
        assert!(reset_label(i64::MAX).is_none());
    }

    #[test]
    fn millisecond_epochs_are_normalized() {
        let secs = Local::now().timestamp();
        assert_eq!(reset_label(secs), reset_label(secs * 1000));
    }
}
