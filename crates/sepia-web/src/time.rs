//! Relative-time helpers. The parser handles the node's ISO-8601
//! timestamps (`YYYY-MM-DDTHH:MM:SS.mmmZ`); display is a short
//! `"3m ago"` style.

/// Parse `"2026-10-08T06:40:34.123Z"` (fractional seconds and `Z`
/// optional) into epoch milliseconds. `None` on anything else — the UI
/// falls back to showing the raw string.
pub fn parse_iso_ms(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { s.get(from..to)?.parse().ok() };
    let year = num(0, 4)?;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    let hour = num(11, 13)?;
    let minute = num(14, 16)?;
    let second = num(17, 19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Fractional seconds, then a required `Z` or ±HH:MM offset — the
    // node always emits `Z`; offsets are accepted leniently by ignoring
    // them (display-only precision).
    let mut frac_ms = 0.0;
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: usize = frac.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let scale = 3_i32.saturating_sub(i32::try_from(digits).unwrap_or(i32::MAX));
        frac_ms = frac[..digits].parse::<f64>().ok()? * 10f64.powi(scale);
        rest = &frac[digits..];
    }
    if rest != "Z" && !rest.starts_with('+') && !rest.starts_with('-') {
        return None;
    }

    // Days since epoch — Howard Hinnant's civil-to-days algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    let ms = days as f64 * 86_400_000.0
        + hour as f64 * 3_600_000.0
        + minute as f64 * 60_000.0
        + second as f64 * 1_000.0
        + frac_ms;
    Some(ms)
}

/// `"just now"`, `"45s ago"`, `"12m ago"`, `"3h ago"`, `"5d ago"`,
/// then a short `"MMM d"` date.
pub fn relative(iso: &str, now_ms: f64) -> String {
    let Some(then) = parse_iso_ms(iso) else {
        return iso.to_string();
    };
    let delta = now_ms - then;
    if delta < 0.0 {
        return "just now".to_string();
    }
    let secs = (delta / 1000.0) as u64;
    if secs < 5 {
        return "just now".to_string();
    }
    if secs < 60 {
        return format!("{secs}s ago");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 30 {
        return format!("{days}d ago");
    }
    short_date(iso)
}

/// The [`relative`] ladder for epoch-millisecond inputs (checkpoint
/// `createdAt` isn't an ISO string); >30d renders `"Jan 1"`-style.
pub fn relative_ms(ms: f64, now_ms: f64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let delta = now_ms - ms;
    if delta < 0.0 {
        return "just now".to_string();
    }
    let secs = (delta / 1000.0) as u64;
    if secs < 5 {
        return "just now".to_string();
    }
    if secs < 60 {
        return format!("{secs}s ago");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 30 {
        return format!("{days}d ago");
    }
    let (year, month, day) = civil_of_ms(ms);
    match MONTHS.get(month.saturating_sub(1) as usize) {
        Some(m) => format!("{m} {day}, {year}"),
        None => format!("{ms:.0}"),
    }
}

/// Days-since-epoch → `(year, month, day)` — the inverse of the
/// civil-to-days algorithm in [`parse_iso_ms`].
fn civil_of_ms(ms: f64) -> (i64, u32, u32) {
    let z = (ms / 86_400_000.0).floor() as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { y + 1 } else { y }, month as u32, day as u32)
}

fn short_date(iso: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month: usize = iso.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(0);
    let day = iso.get(8..10).unwrap_or("");
    match MONTHS.get(month.saturating_sub(1)) {
        Some(m) => format!("{m} {}", day.trim_start_matches('0')),
        None => iso.get(..10).unwrap_or(iso).to_string(),
    }
}

/// Wall clock in epoch milliseconds — `Date.now()` on wasm,
/// `SystemTime` elsewhere.
pub fn now_ms() -> f64 {
    #[cfg(feature = "hydrate")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(feature = "hydrate"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iso_with_millis() {
        // 1970-01-01T00:00:01.5Z
        assert_eq!(parse_iso_ms("1970-01-01T00:00:01.5Z"), Some(1500.0));
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0.0));
        // 2026-10-08T06:40:34.123Z
        let ms = parse_iso_ms("2026-10-08T06:40:34.123Z").unwrap_or_default();
        assert!((ms - 1_791_441_634_123.0).abs() < 1.0, "got {ms}");
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_iso_ms(""), None);
        assert_eq!(parse_iso_ms("not a date"), None);
        assert_eq!(parse_iso_ms("2026-13-40T99:99:99Z"), None);
    }

    #[test]
    fn relative_windows() {
        let iso = "1970-01-01T00:00:00Z";
        assert_eq!(relative(iso, 0.0), "just now");
        assert_eq!(relative(iso, 45_000.0), "45s ago");
        assert_eq!(relative(iso, 12.0 * 60_000.0), "12m ago");
        assert_eq!(relative(iso, 3.0 * 3_600_000.0), "3h ago");
        assert_eq!(relative(iso, 5.0 * 86_400_000.0), "5d ago");
        // >30d falls back to the event's own short date (Jan 1, 1970).
        assert_eq!(relative(iso, 60.0 * 86_400_000.0), "Jan 1");
    }

    #[test]
    fn relative_ms_matches_the_ladder() {
        assert_eq!(relative_ms(0.0, 0.0), "just now");
        assert_eq!(relative_ms(0.0, 12.0 * 60_000.0), "12m ago");
        // 2026-10-08T06:40:34.123Z rendered long after the fact.
        assert_eq!(
            relative_ms(
                1_791_441_634_123.0,
                1_791_441_634_123.0 + 60.0 * 86_400_000.0
            ),
            "Oct 8, 2026"
        );
    }
}
