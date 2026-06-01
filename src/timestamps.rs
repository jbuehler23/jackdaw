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
