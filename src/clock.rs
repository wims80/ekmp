use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn unix_time_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

/// Seconds since the Unix epoch for an RFC 7231 HTTP date.
pub(crate) fn http_date(value: &str) -> Option<u64> {
    httpdate::parse_http_date(value)
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}
