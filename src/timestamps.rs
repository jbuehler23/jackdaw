use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FALLBACK_UTC: &str = "1970-01-01T00:00:00Z";
const SECONDS_PER_DAY: u64 = 86_400;

/// Current UTC instant as RFC 3339 (`2026-06-01T12:34:56Z`).
pub fn utc_rfc3339_now() -> String {
    format_rfc3339(SystemTime::now())
}

/// The current month in UTC, 1 for January.
pub fn utc_month_now() -> u32 {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() / SECONDS_PER_DAY);
    civil_from_days(days as i64).1
}

/// How long ago an RFC 3339 instant was, in the coarse form a list
/// renders (`just now`, `3 hours ago`, `2 days ago`). `None` when the
/// string is missing or unparseable, so callers can omit the column
/// rather than print a placeholder.
pub fn relative_to_now(rfc3339: &str) -> Option<String> {
    let then = parse_rfc3339(rfc3339)?;
    let seconds = SystemTime::now().duration_since(then).ok()?.as_secs();
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

fn format_rfc3339(instant: SystemTime) -> String {
    let Ok(since_epoch) = instant.duration_since(UNIX_EPOCH) else {
        return FALLBACK_UTC.to_string();
    };
    let seconds = since_epoch.as_secs();
    let (year, month, day) = civil_from_days((seconds / SECONDS_PER_DAY) as i64);
    let of_day = seconds % SECONDS_PER_DAY;
    let mut text = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
    );
    let nanos = since_epoch.subsec_nanos();
    if nanos != 0 {
        text.push('.');
        text.push_str(format!("{nanos:09}").trim_end_matches('0'));
    }
    text.push('Z');
    text
}

fn parse_rfc3339(text: &str) -> Option<SystemTime> {
    let digits = |field: &str| field.bytes().all(|byte| byte.is_ascii_digit());
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        text.get(range)
            .filter(|field| digits(field))
            .and_then(|field| field.parse().ok())
    };
    let bytes = text.as_bytes();
    let separators = [(4, b'-'), (7, b'-'), (13, b':'), (16, b':')];
    if bytes.len() < 20
        || separators.iter().any(|&(at, byte)| bytes[at] != byte)
        || !matches!(bytes[10], b'T' | b't')
    {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }

    let mut rest = text.get(19..)?;
    let mut nanos = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let length = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if length == 0 {
            return None;
        }
        let kept = &fraction[..length.min(9)];
        nanos = kept.parse::<u32>().ok()? * 10u32.pow(9 - kept.len() as u32);
        rest = &fraction[length..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ if rest.len() == 6 && rest.as_bytes()[3] == b':' => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hours: i64 = rest.get(1..3).filter(|f| digits(f))?.parse().ok()?;
            let minutes: i64 = rest.get(4..6).filter(|f| digits(f))?.parse().ok()?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            sign * (hours * 3600 + minutes * 60)
        }
        _ => return None,
    };

    let seconds = days_from_civil(year, month, day) * SECONDS_PER_DAY as i64
        + hour * 3600
        + minute * 60
        + second
        - offset;
    let whole = Duration::from_secs(seconds.unsigned_abs());
    let instant = if seconds >= 0 {
        UNIX_EPOCH.checked_add(whole)
    } else {
        UNIX_EPOCH.checked_sub(whole)
    };
    instant?.checked_add(Duration::from_nanos(u64::from(nanos)))
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The `(year, month, day)` a count of days since 1970-01-01 falls on, after
/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Days since 1970-01-01 of a civil date, the inverse of [`civil_from_days`].
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64, nanos: u32) -> SystemTime {
        UNIX_EPOCH + Duration::new(seconds, nanos)
    }

    fn ago(seconds: u64) -> String {
        let then = SystemTime::now() - Duration::from_secs(seconds);
        relative_to_now(&format_rfc3339(then)).expect("formats")
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
        assert_eq!(relative_to_now("2026-02-30T00:00:00Z"), None);
        assert_eq!(relative_to_now("2026-06-01T12:34:56"), None);
        assert_eq!(relative_to_now("2026-06-01T24:00:00Z"), None);
    }

    #[test]
    fn instants_format_as_utc_with_trimmed_fractions() {
        assert_eq!(format_rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(at(1_780_317_296, 0)), "2026-06-01T12:34:56Z");
        assert_eq!(
            format_rfc3339(at(1_709_251_199, 120_000_000)),
            "2024-02-29T23:59:59.12Z"
        );
        assert_eq!(
            format_rfc3339(at(946_677_600, 700)),
            "1999-12-31T22:00:00.0000007Z"
        );
    }

    #[test]
    fn formatted_instants_parse_back() {
        for instant in [
            UNIX_EPOCH,
            at(1_780_317_296, 0),
            at(1_709_251_199, 120_000_000),
            at(946_677_600, 7),
        ] {
            assert_eq!(parse_rfc3339(&format_rfc3339(instant)), Some(instant));
        }
    }

    #[test]
    fn offsets_are_folded_into_utc() {
        assert_eq!(
            parse_rfc3339("2026-06-01T14:34:56+02:00"),
            Some(at(1_780_317_296, 0))
        );
        assert_eq!(
            parse_rfc3339("1999-12-31T17:00:00-05:00"),
            Some(at(946_677_600, 0))
        );
    }
}
