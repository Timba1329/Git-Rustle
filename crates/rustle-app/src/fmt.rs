//! Date formatting in the local time zone.

use chrono::{DateTime, Local, Utc};

fn local(ts: i64) -> Option<DateTime<Local>> {
    DateTime::<Utc>::from_timestamp(ts, 0).map(|d| d.with_timezone(&Local))
}

/// `2024-05-03` or `2024-05-03 14:07`.
pub fn date(ts: i64, with_time: bool) -> String {
    match local(ts) {
        Some(d) if with_time => d.format("%Y-%m-%d %H:%M").to_string(),
        Some(d) => d.format("%Y-%m-%d").to_string(),
        None => String::new(),
    }
}

/// "5 min ago", "3 h ago", "2 d ago"; falls back to the date after a week.
pub fn relative(ts: i64) -> String {
    let now = Utc::now().timestamp();
    let d = now - ts;
    match d {
        i64::MIN..=-1 => date(ts, false),
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => format!("{} h ago", d / 3600),
        86_400..=604_799 => format!("{} d ago", d / 86_400),
        _ => date(ts, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_buckets() {
        let now = Utc::now().timestamp();
        assert_eq!(relative(now), "just now");
        assert_eq!(relative(now - 300), "5 min ago");
        assert_eq!(relative(now - 7200), "2 h ago");
        assert_eq!(relative(now - 3 * 86_400), "3 d ago");
        assert_eq!(relative(0).len(), 10);
    }

    #[test]
    fn date_has_expected_shape() {
        let s = date(1_700_000_000, true);
        assert_eq!(s.len(), 16);
        assert!(s.starts_with("2023-11-1"));
    }
}
