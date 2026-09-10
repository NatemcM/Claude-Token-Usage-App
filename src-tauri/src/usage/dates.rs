use chrono::{DateTime, Datelike, FixedOffset, Local, Offset, TimeZone, Timelike, Utc};

/// Parse a transcript `timestamp` (ISO8601, always UTC with a Z suffix in
/// observed data) into epoch milliseconds.
pub fn parse_iso8601_ms(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// The machine's current UTC offset, in minutes east of UTC.
pub fn current_tz_offset_minutes() -> i32 {
    Local::now().offset().fix().local_minus_utc() / 60
}

fn shifted(ts_ms: i64, offset_minutes: i32) -> DateTime<FixedOffset> {
    let offset = FixedOffset::east_opt(offset_minutes * 60)
        .unwrap_or_else(|| FixedOffset::east_opt(0).expect("utc offset"));
    offset.timestamp_millis_opt(ts_ms).single().unwrap_or_else(|| {
        FixedOffset::east_opt(0)
            .expect("utc offset")
            .timestamp_millis_opt(0)
            .single()
            .expect("epoch")
    })
}

/// "YYYY-MM-DD" in the given offset. Exposed for deterministic tests.
pub fn date_key_with_offset(ts_ms: i64, offset_minutes: i32) -> String {
    let d = shifted(ts_ms, offset_minutes);
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// "YYYY-MM" in the given offset.
pub fn month_prefix_with_offset(ts_ms: i64, offset_minutes: i32) -> String {
    let d = shifted(ts_ms, offset_minutes);
    format!("{:04}-{:02}", d.year(), d.month())
}

pub fn hour_with_offset(ts_ms: i64, offset_minutes: i32) -> u32 {
    shifted(ts_ms, offset_minutes).hour()
}

/// Day key in the machine's current local timezone.
pub fn local_date_key(ts_ms: i64) -> String {
    date_key_with_offset(ts_ms, current_tz_offset_minutes())
}

/// Month prefix in the machine's current local timezone.
pub fn local_month_prefix(ts_ms: i64) -> String {
    month_prefix_with_offset(ts_ms, current_tz_offset_minutes())
}

pub fn local_hour(ts_ms: i64) -> u32 {
    hour_with_offset(ts_ms, current_tz_offset_minutes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_transcript_timestamp_to_epoch_ms() {
        // Real timestamp shape from an assistant record.
        let ms = parse_iso8601_ms("2026-09-10T08:32:59.076Z").expect("parse");
        assert_eq!(ms, 1789029179076);
    }

    #[test]
    fn parses_timestamp_without_fractional_seconds() {
        let ms = parse_iso8601_ms("2026-09-10T08:32:59Z").expect("parse");
        assert_eq!(ms, 1789029179000);
    }

    #[test]
    fn rejects_garbage_timestamp() {
        assert!(parse_iso8601_ms("not-a-date").is_none());
        assert!(parse_iso8601_ms("").is_none());
    }

    #[test]
    fn utc_timestamp_near_midnight_lands_in_correct_local_day() {
        // 2026-09-10T20:00:00Z. In UTC+07 that is 2026-09-11 03:00 local, so
        // the day key must differ from the UTC date. Uses an explicit offset
        // so the test does not depend on the machine's timezone.
        let ms = parse_iso8601_ms("2026-09-10T20:00:00Z").expect("parse");
        assert_eq!(date_key_with_offset(ms, 420), "2026-09-11");
        assert_eq!(date_key_with_offset(ms, 0), "2026-09-10");
        // UTC-07 pushes it earlier still, but same date here.
        assert_eq!(date_key_with_offset(ms, -420), "2026-09-10");
    }

    #[test]
    fn month_prefix_respects_offset_at_month_boundary() {
        // 2026-08-31T20:00:00Z is 2026-09-01 local at UTC+07: month rolls over.
        let ms = parse_iso8601_ms("2026-08-31T20:00:00Z").expect("parse");
        assert_eq!(month_prefix_with_offset(ms, 420), "2026-09");
        assert_eq!(month_prefix_with_offset(ms, 0), "2026-08");
    }

    #[test]
    fn local_hour_respects_offset() {
        let ms = parse_iso8601_ms("2026-09-10T20:00:00Z").expect("parse");
        assert_eq!(hour_with_offset(ms, 420), 3);
        assert_eq!(hour_with_offset(ms, 0), 20);
    }
}
