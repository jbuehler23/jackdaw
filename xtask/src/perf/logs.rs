//! The editor's output as timed lines, and the log timestamps in a game's.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One line a process printed, with when the probe read it.
#[derive(Clone, Debug)]
pub struct Line {
    pub at: Instant,
    pub text: String,
}

/// Lines read so far, shared with the threads reading them.
pub type Lines = Arc<Mutex<Vec<Line>>>;

/// Read `source` line by line on its own thread into `lines`, copying each to
/// `copy`.
pub fn capture(
    source: impl Read + Send + 'static,
    lines: Lines,
    copy: Arc<Mutex<std::fs::File>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let reader = BufReader::new(source);
        for line in reader.split(b'\n').map_while(Result::ok) {
            let text = strip_ansi(&String::from_utf8_lossy(&line));
            if let Ok(mut file) = copy.lock() {
                let _ = writeln!(file, "{text}");
            }
            if let Ok(mut lines) = lines.lock() {
                lines.push(Line {
                    at: Instant::now(),
                    text,
                });
            }
        }
    })
}

/// `text` without terminal colour sequences.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

/// The UTC timestamp a log line starts with, as `2026-11-10T18:39:59.414769Z`.
pub fn line_time(text: &str) -> Option<SystemTime> {
    let stamp = text.split_whitespace().next()?;
    let stamp = stamp.strip_suffix('Z')?;
    let (date, time) = stamp.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut clock = clock.splitn(3, ':').map(str::parse::<u64>);
    let (hour, minute, second) = (
        clock.next()?.ok()?,
        clock.next()?.ok()?,
        clock.next()?.ok()?,
    );
    let nanos: u32 = format!("{fraction:0<9}").get(..9)?.parse().ok()?;
    let days = u64::try_from(days_from_civil(year, month, day)).ok()?;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    Some(UNIX_EPOCH + Duration::new(seconds, nanos))
}

/// `at` as `2026-11-10T18:39:59Z`.
pub fn format_utc(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let rest = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date `days` after 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_sequences_are_dropped_and_text_kept() {
        let coloured = "\u{1b}[2m2026-11-10T18:39:59.414769Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m PIE: Play#1 connected";
        assert_eq!(
            strip_ansi(coloured),
            "2026-11-10T18:39:59.414769Z  INFO PIE: Play#1 connected"
        );
    }

    #[test]
    fn a_log_timestamp_reads_as_utc_with_its_fraction() {
        let at = line_time("2026-11-10T18:39:59.414769Z  INFO Loaded 43 asset files").unwrap();
        let since = at.duration_since(UNIX_EPOCH).unwrap();
        assert_eq!(since.as_secs(), 1_794_335_999);
        assert_eq!(since.subsec_micros(), 414_769);
    }

    #[test]
    fn a_line_without_a_timestamp_has_none() {
        assert_eq!(line_time("thread 'main' panicked"), None);
        assert_eq!(line_time(""), None);
    }

    #[test]
    fn dates_round_trip_through_day_counts() {
        for days in [-1, 0, 59, 60, 365, 11_016, 20_767, 20_768] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn a_time_formats_as_utc_to_the_second() {
        let at = UNIX_EPOCH + Duration::from_secs(1_794_335_999);
        assert_eq!(format_utc(at), "2026-11-10T18:39:59Z");
    }
}
