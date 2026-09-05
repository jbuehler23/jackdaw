use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const FALLBACK_UTC: &str = "1970-01-01T00:00:00Z";

/// Current UTC instant as RFC 3339 (`2026-06-01T12:34:56Z`).
pub fn utc_rfc3339_now() -> String {
    match OffsetDateTime::now_utc().format(&Rfc3339) {
        Ok(formatted) => formatted,
        Err(_) => FALLBACK_UTC.to_string(),
    }
}

/// How long ago an RFC 3339 instant was, in the coarse form a list
/// renders (`just now`, `3 hours ago`, `2 days ago`). `None` when the
/// string is missing or unparseable, so callers can omit the column
/// rather than print a placeholder.
pub fn relative_to_now(rfc3339: &str) -> Option<String> {
    let then = OffsetDateTime::parse(rfc3339, &Rfc3339).ok()?;
    let seconds = (OffsetDateTime::now_utc() - then).whole_seconds();
    if seconds < 0 {
        return None;
    }
    let (count, unit) = match seconds {
        ..60 => return Some("just now".to_string()),
        60..3600 => (seconds / 60, "minute"),
        3600..86_400 => (seconds / 3600, "hour"),
        86_400..2_592_000 => (seconds / 86_400, "day"),
        2_592_000..31_536_000 => (seconds / 2_592_000, "month"),
        _ => (seconds / 31_536_000, "year"),
    };
    let plural = if count == 1 { "" } else { "s" };
    Some(format!("{count} {unit}{plural} ago"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ago(seconds: i64) -> String {
        let then = OffsetDateTime::now_utc() - time::Duration::seconds(seconds);
        relative_to_now(&then.format(&Rfc3339).unwrap()).expect("formats")
    }

    #[test]
    fn coarse_buckets_read_naturally() {
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(3 * 3600), "3 hours ago");
        assert_eq!(ago(2 * 86_400), "2 days ago");
        assert_eq!(ago(40 * 86_400), "1 month ago");
        assert_eq!(ago(400 * 86_400), "1 year ago");
    }

    #[test]
    fn an_unusable_timestamp_yields_nothing_to_render() {
        assert_eq!(relative_to_now(""), None);
        assert_eq!(relative_to_now("not a date"), None);
    }
}
