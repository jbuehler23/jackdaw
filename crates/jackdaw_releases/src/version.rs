//! Editor version names: `0.20.1` on Stable, `nightly-2026-10-11` on
//! Bleeding Edge.

use std::cmp::Ordering;

const NIGHTLY_PREFIX: &str = "nightly-";

/// The `major.minor` line a stable version belongs to, e.g. `0.20` for
/// `0.20.1` or `0.20.0-rc.2`. `None` for anything that is not semver.
pub fn line_of(version: &str) -> Option<String> {
    let parsed = semver::Version::parse(version).ok()?;
    Some(format!("{}.{}", parsed.major, parsed.minor))
}

/// Whether `query` names a line (`0.20`) rather than a version.
pub fn is_line(query: &str) -> bool {
    let mut parts = query.split('.');
    let numeric = |part: Option<&str>| {
        part.is_some_and(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    };
    numeric(parts.next()) && numeric(parts.next()) && parts.next().is_none()
}

/// The date of a nightly build, as days since 1970-01-01.
pub fn nightly_day(version: &str) -> Option<i64> {
    let date = version.strip_prefix(NIGHTLY_PREFIX)?;
    parse_date(date)
}

/// The id of the nightly built on `date` (`YYYY-MM-DD`).
pub fn nightly_name(date: &str) -> String {
    format!("{NIGHTLY_PREFIX}{date}")
}

/// Whether `version` names a nightly build.
pub fn is_nightly(version: &str) -> bool {
    nightly_day(version).is_some()
}

/// Order two version names: semver versions by precedence, nightlies by
/// date, and anything else by text.
pub fn compare(a: &str, b: &str) -> Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

/// Days since 1970-01-01 of a `YYYY-MM-DD` date.
pub fn parse_date(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

/// Today in UTC, as days since 1970-01-01.
pub fn today() -> i64 {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    i64::try_from(seconds / 86_400).unwrap_or_default()
}

/// The current UTC time in RFC 3339, to the second.
pub fn now_rfc3339() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    format_rfc3339(i64::try_from(seconds).unwrap_or_default())
}

/// Seconds since 1970-01-01 UTC as RFC 3339, e.g. `2026-10-11T06:00:00Z`.
pub fn format_rfc3339(seconds: i64) -> String {
    let time = seconds.rem_euclid(86_400);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        format_date(seconds.div_euclid(86_400)),
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

/// A day number as `YYYY-MM-DD`.
pub fn format_date(days: i64) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant's `days_from_civil`, proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_is_major_and_minor_including_prereleases() {
        assert_eq!(line_of("0.20.1").as_deref(), Some("0.20"));
        assert_eq!(line_of("0.19.0-rc.10").as_deref(), Some("0.19"));
        assert_eq!(line_of("nightly-2026-10-11"), None);
    }

    #[test]
    fn line_queries_are_two_numbers() {
        assert!(is_line("0.20"));
        assert!(!is_line("0.20.1"));
        assert!(!is_line("stable"));
        assert!(!is_line("0."));
    }

    #[test]
    fn versions_order_by_semver_precedence() {
        assert_eq!(compare("0.20.10", "0.20.9"), Ordering::Greater);
        assert_eq!(compare("0.20.0-rc.1", "0.20.0"), Ordering::Less);
        assert_eq!(
            compare("nightly-2026-10-11", "nightly-2026-09-30"),
            Ordering::Greater
        );
    }

    #[test]
    fn dates_round_trip_through_day_numbers() {
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("2000-03-01"), Some(11_017));
        for day in [0, 11_017, 20_372, 20_742] {
            assert_eq!(parse_date(&format_date(day)), Some(day));
        }
        assert_eq!(nightly_day("nightly-1970-01-31"), Some(30));
        assert_eq!(parse_date("2026-13-01"), None);
        assert_eq!(format_rfc3339(86_400 + 3661), "1970-01-02T01:01:01Z");
    }
}
