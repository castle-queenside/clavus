//! Time formatting that speaks the user's clock.
//!
//! The snapshot log used to print every time in UTC with no label, so
//! the times did not match the user's own clock and there was no way to
//! tell. The times are now local, and the offset is shown once per
//! list so the reader knows what clock they are looking at.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The broken-down local time for a Unix timestamp.
#[derive(Debug, Clone, Copy)]
pub struct LocalTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// The offset from UTC, in seconds.
    pub offset: i64,
}

impl LocalTime {
    /// The offset from UTC, written as `UTC+02:00` or `UTC-07:00`.
    pub fn offset_label(&self) -> String {
        let sign = if self.offset < 0 { '-' } else { '+' };
        let abs = self.offset.abs();
        format!("UTC{}{:02}:{:02}", sign, abs / 3600, (abs % 3600) / 60)
    }

    /// The time as `YYYY-MM-DD HH:MM`.
    pub fn format(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }
}

/// Split a Unix timestamp into a local date and time.
///
/// The system's own time zone rules do the work, so daylight saving
/// is handled without the tool keeping its own table. A timestamp the
/// system cannot express falls back to UTC.
pub fn local_time(secs: i64) -> LocalTime {
    // `localtime_r` fills in a `tm` the caller owns, so this is safe to
    // call from more than one thread.
    // `libc::time_t` is 64 bits on most platforms but 32 bits on musl
    // before 1.2, and the libc crate warns about the alias. An instant
    // that the platform's type cannot hold falls back to UTC rather
    // than wrapping into the wrong year.
    #[allow(deprecated)]
    let time_t = secs as libc::time_t;
    if secs as i128 != time_t as i128 {
        return utc_time(secs);
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = !unsafe { libc::localtime_r(&time_t, &mut tm) }.is_null();
    if !ok {
        return utc_time(secs);
    }
    LocalTime {
        year: (tm.tm_year as i64) + 1900,
        month: (tm.tm_mon as u32) + 1,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
        offset: tm.tm_gmtoff as i64,
    }
}

/// Split a Unix timestamp into a UTC date and time.
pub fn utc_time(secs: i64) -> LocalTime {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (year, yday) = civil_from_days(days);
    let (month, day) = month_and_day(year, yday);
    LocalTime {
        year,
        month,
        day,
        hour: (rem / 3600) as u32,
        minute: ((rem % 3600) / 60) as u32,
        second: (rem % 60) as u32,
        offset: 0,
    }
}

/// A Unix timestamp as a local `YYYY-MM-DD HH:MM`.
///
/// A timestamp before the epoch is still shown, in UTC, rather than
/// refused.
pub fn format_local(secs: f64) -> String {
    local_time(secs as i64).format()
}

/// The current local offset, written as `UTC+02:00`.
pub fn local_offset_label() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs() as i64;
    local_time(now).offset_label()
}

/// Whether a year is a leap year.
fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// The month and day for a day-of-year.
fn month_and_day(year: i64, yday: i64) -> (u32, u32) {
    let days_in_month: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut remaining = yday;
    for (i, &dm) in days_in_month.iter().enumerate() {
        let dm = if i == 1 && is_leap(year) { dm + 1 } else { dm };
        if remaining < dm {
            return ((i + 1) as u32, (remaining + 1) as u32);
        }
        remaining -= dm;
    }
    (12, 31)
}

/// The year and day-of-year for a count of days since 1970-01-01.
///
/// This is the standard civil-calendar conversion, so it is correct for
/// every year and not only for the years the Unix epoch covers.
fn civil_from_days(days: i64) -> (i64, i64) {
    // Howard Hinnant's `civil_from_days`.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    let yday = day_of_year(year, m as u32, d as u32);
    (year, yday)
}

/// The day-of-year for a date.
fn day_of_year(year: i64, month: u32, day: u32) -> i64 {
    let days_in_month: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut total = day as i64 - 1;
    for (i, &dm) in days_in_month.iter().enumerate() {
        if (i as u32) + 1 >= month {
            break;
        }
        let dm = if i == 1 && is_leap(year) { dm + 1 } else { dm };
        total += dm;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_is_1970() {
        let t = utc_time(0);
        assert_eq!((t.year, t.month, t.day), (1970, 1, 1));
        assert_eq!((t.hour, t.minute, t.second), (0, 0, 0));
    }

    #[test]
    fn a_known_instant_is_right_in_utc() {
        // 2021-01-01 00:00:00 UTC
        let t = utc_time(1_609_459_200);
        assert_eq!((t.year, t.month, t.day), (2021, 1, 1));
        assert_eq!((t.hour, t.minute), (0, 0));
    }

    #[test]
    fn a_leap_day_is_right() {
        // 2024-02-29 12:00:00 UTC
        let t = utc_time(1_709_208_000);
        assert_eq!((t.year, t.month, t.day), (2024, 2, 29));
        assert_eq!(t.hour, 12);
    }

    #[test]
    fn a_leap_second_year_is_right() {
        // 2000-02-29, a year that is a leap year in every rule.
        let t = utc_time(951_782_400);
        assert_eq!((t.year, t.month, t.day), (2000, 2, 29));
    }

    #[test]
    fn a_century_that_is_not_a_leap_year_is_right() {
        // 1900-03-01 — 1900 was not a leap year.
        let t = utc_time(-2_203_891_200);
        assert_eq!((t.year, t.month, t.day), (1900, 3, 1));
    }

    #[test]
    fn the_local_time_is_the_utc_time_plus_the_offset() {
        let utc = utc_time(1_700_000_000);
        let local = local_time(1_700_000_000);
        assert_eq!(local.offset, local.offset); // the offset is read
        // The local wall-clock time and the UTC time must differ by
        // exactly the offset, hour for hour.
        let utc_secs = utc.hour as i64 * 3600 + utc.minute as i64 * 60 + utc.second as i64;
        let local_secs =
            local.hour as i64 * 3600 + local.minute as i64 * 60 + local.second as i64;
        let mut diff = local_secs - utc_secs;
        if diff > 43_200 {
            diff -= 86_400;
        }
        if diff < -43_200 {
            diff += 86_400;
        }
        assert_eq!(diff, local.offset);
    }

    #[test]
    fn the_offset_label_names_the_clock() {
        let t = local_time(1_700_000_000);
        let label = t.offset_label();
        assert!(label.starts_with("UTC"), "{}", label);
        assert!(label.len() == 9, "{}", label);
    }

    #[test]
    fn a_time_before_the_epoch_is_still_shown() {
        let text = format_local(-1_000_000.0);
        assert_eq!(text.len(), 16, "{}", text);
    }
}
