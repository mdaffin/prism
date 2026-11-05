//! Showing how long ago something happened.

/// How long before `now` a timestamp such as `2026-01-02T03:04:05Z` was, like `3 hours ago`.
/// A timestamp that can't be read is shown as it is.
pub fn ago(timestamp: &str, now: jiff::Timestamp) -> String {
    let Ok(then) = timestamp.parse::<jiff::Timestamp>() else {
        return timestamp.to_string();
    };
    let seconds = now.as_second() - then.as_second();
    let units = [
        (365 * 24 * 3600, "year"),
        (30 * 24 * 3600, "month"),
        (7 * 24 * 3600, "week"),
        (24 * 3600, "day"),
        (3600, "hour"),
        (60, "minute"),
    ];
    for (size, unit) in units {
        if seconds >= size {
            let n = seconds / size;
            return format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
        }
    }
    "just now".to_string()
}

/// How long before `now` a timestamp was, in the fewest characters: `5m`, `2h`, `3d`, `2w`, `4mo`,
/// `1y`. Under a minute is `now`. A timestamp that can't be read is shown as it is.
pub fn short_age(timestamp: &str, now: jiff::Timestamp) -> String {
    let Ok(then) = timestamp.parse::<jiff::Timestamp>() else {
        return timestamp.to_string();
    };
    let seconds = now.as_second() - then.as_second();
    let units = [
        (365 * 24 * 3600, "y"),
        (30 * 24 * 3600, "mo"),
        (7 * 24 * 3600, "w"),
        (24 * 3600, "d"),
        (3600, "h"),
        (60, "m"),
    ];
    for (size, unit) in units {
        if seconds >= size {
            return format!("{}{unit}", seconds / size);
        }
    }
    "now".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanises_times() {
        let now: jiff::Timestamp = "2026-03-10T12:00:00Z".parse().unwrap();
        let at = |timestamp| ago(timestamp, now);
        assert_eq!(at("2026-03-10T12:00:00Z"), "just now");
        assert_eq!(at("2026-03-10T11:59:01Z"), "just now");
        assert_eq!(at("2026-03-10T11:59:00Z"), "1 minute ago");
        assert_eq!(at("2026-03-10T11:15:00Z"), "45 minutes ago");
        assert_eq!(at("2026-03-10T09:00:00Z"), "3 hours ago");
        assert_eq!(at("2026-03-09T12:00:00Z"), "1 day ago");
        assert_eq!(at("2026-03-01T12:00:00Z"), "1 week ago");
        assert_eq!(at("2026-01-10T12:00:00Z"), "1 month ago");
        assert_eq!(at("2024-03-10T12:00:00Z"), "2 years ago");
        // The future, such as a clock that is a little off, isn't an error.
        assert_eq!(at("2026-03-10T12:00:30Z"), "just now");
        assert_eq!(at("yesterday"), "yesterday");
    }

    #[test]
    fn shortens_times_to_a_number_and_a_unit() {
        let now: jiff::Timestamp = "2026-03-10T12:00:00Z".parse().unwrap();
        let short = |timestamp| short_age(timestamp, now);
        assert_eq!(short("2026-03-10T11:59:30Z"), "now");
        assert_eq!(short("2026-03-10T11:15:00Z"), "45m");
        assert_eq!(short("2026-03-10T09:00:00Z"), "3h");
        assert_eq!(short("2026-03-07T12:00:00Z"), "3d");
        assert_eq!(short("2026-02-24T12:00:00Z"), "2w");
        assert_eq!(short("2025-12-10T12:00:00Z"), "3mo");
        assert_eq!(short("2024-03-10T12:00:00Z"), "2y");
        assert_eq!(short("2026-03-10T12:00:30Z"), "now");
        assert_eq!(short("yesterday"), "yesterday");
    }
}
