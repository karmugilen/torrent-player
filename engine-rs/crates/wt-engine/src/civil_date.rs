//! Pure-Rust UTC civil-date conversion matching libc `gmtime_r` / `timegm`
//! for the Gregorian calendar (proleptic). Used by the tracker cache so
//! Android, Linux, Windows, and macOS serialize identical RFC3339 timestamps.

/// Broken-down UTC date/time fields, parallel to `libc::tm` without timezone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilDateTime {
    pub year: i32,  // full year, e.g. 2021
    pub month: i32, // 1..=12
    pub day: i32,   // 1..=31
    pub hour: i32,  // 0..=23
    pub minute: i32,
    pub second: i32, // 0..=59 (no leap seconds)
}

/// Convert Unix seconds to UTC civil fields. Returns `None` only for values
/// that overflow the intermediate day arithmetic (far outside practical use).
pub fn civil_from_unix(seconds: i64) -> Option<CivilDateTime> {
    let days = seconds.div_euclid(86_400);
    let tod = seconds.rem_euclid(86_400) as i32;
    let (year, month, day) = civil_from_days(days)?;
    Some(CivilDateTime {
        year,
        month,
        day,
        hour: tod / 3600,
        minute: (tod % 3600) / 60,
        second: tod % 60,
    })
}

/// Convert UTC civil fields to Unix seconds (like `timegm`). Returns `None`
/// when the calendar date is invalid (e.g. Feb 30) or the result overflows.
pub fn unix_from_civil(dt: CivilDateTime) -> Option<i64> {
    if !(1..=12).contains(&dt.month)
        || dt.hour < 0
        || dt.hour > 23
        || dt.minute < 0
        || dt.minute > 59
        || dt.second < 0
        || dt.second > 59
        || dt.day < 1
        || dt.day > days_in_month(dt.year, dt.month)?
    {
        return None;
    }
    let days = days_from_civil(dt.year, dt.month, dt.day)?;
    days.checked_mul(86_400)?
        .checked_add(i64::from(dt.hour) * 3600 + i64::from(dt.minute) * 60 + i64::from(dt.second))
}

/// Howard Hinnant civil-from-days (proleptic Gregorian), days relative to
/// 1970-01-01.
fn civil_from_days(days: i64) -> Option<(i32, i32, i32)> {
    // Shift so era 0 starts at 0000-03-01.
    let z = days.checked_add(719_468)?;
    let era = if z >= 0 {
        z / 146_097
    } else {
        (z - 146_096) / 146_097
    };
    let doe = (z - era * 146_097) as u32; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = (era * 400) as i64 + yoe as i64;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    let year = i32::try_from(year).ok()?;
    Some((year, m as i32, d as i32))
}

fn days_from_civil(year: i32, month: i32, day: i32) -> Option<i64> {
    let y = if month <= 2 {
        i64::from(year) - 1
    } else {
        i64::from(year)
    };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = (y - era * 400) as u32;
    let mp = if month > 2 { month - 3 } else { month + 9 } as u32;
    let doy = (153 * mp + 2) / 5 + day as u32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe as i64).checked_sub(719_468)
}

fn days_in_month(year: i32, month: i32) -> Option<i32> {
    const DAYS: [i32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&month) {
        return None;
    }
    let mut d = DAYS[(month - 1) as usize];
    if month == 2 && is_leap(year) {
        d = 29;
    }
    Some(d)
}

fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known Unix timestamps and their UTC civil forms (must match gmtime_r).
    const CASES: &[(i64, CivilDateTime)] = &[
        (
            0,
            CivilDateTime {
                year: 1970,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            },
        ),
        (
            1,
            CivilDateTime {
                year: 1970,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 1,
            },
        ),
        (
            -1,
            CivilDateTime {
                year: 1969,
                month: 12,
                day: 31,
                hour: 23,
                minute: 59,
                second: 59,
            },
        ),
        (
            946_684_800,
            CivilDateTime {
                year: 2000,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            },
        ),
        (
            1_609_459_200,
            CivilDateTime {
                year: 2021,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            },
        ),
        (
            1_700_000_000,
            CivilDateTime {
                year: 2023,
                month: 11,
                day: 14,
                hour: 22,
                minute: 13,
                second: 20,
            },
        ),
        (
            2_147_483_647,
            CivilDateTime {
                year: 2038,
                month: 1,
                day: 19,
                hour: 3,
                minute: 14,
                second: 7,
            },
        ),
        (
            253_402_300_799,
            CivilDateTime {
                year: 9999,
                month: 12,
                day: 31,
                hour: 23,
                minute: 59,
                second: 59,
            },
        ),
        (
            -62_135_596_800,
            CivilDateTime {
                year: 1,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            },
        ),
        // Leap day
        (
            1_582_934_400,
            CivilDateTime {
                year: 2020,
                month: 2,
                day: 29,
                hour: 0,
                minute: 0,
                second: 0,
            },
        ),
    ];

    #[test]
    fn known_timestamps_round_trip() {
        for &(ts, expect) in CASES {
            let got = civil_from_unix(ts).expect("civil_from_unix");
            assert_eq!(got, expect, "civil_from_unix({ts})");
            let back = unix_from_civil(got).expect("unix_from_civil");
            assert_eq!(back, ts, "unix_from_civil({got:?})");
        }
    }

    #[test]
    fn rejects_invalid_calendar_dates() {
        assert!(unix_from_civil(CivilDateTime {
            year: 2021,
            month: 2,
            day: 29,
            hour: 0,
            minute: 0,
            second: 0,
        })
        .is_none());
        assert!(unix_from_civil(CivilDateTime {
            year: 2021,
            month: 13,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        })
        .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn matches_libc_gmtime_r_and_timegm() {
        for &(ts, _) in CASES {
            // Skip extremes that some 32-bit time_t platforms cannot represent.
            if ts < i64::from(i32::MIN) || ts > i64::from(i32::MAX) {
                // Still compare on 64-bit time_t hosts via libc::time_t size.
                if std::mem::size_of::<libc::time_t>() < 8 {
                    continue;
                }
            }
            let seconds = ts as libc::time_t;
            if seconds as i64 != ts {
                continue;
            }
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            assert!(
                !unsafe { libc::gmtime_r(&seconds, &mut tm) }.is_null(),
                "gmtime_r({ts})"
            );
            let libc_civil = CivilDateTime {
                year: tm.tm_year + 1900,
                month: tm.tm_mon + 1,
                day: tm.tm_mday,
                hour: tm.tm_hour,
                minute: tm.tm_min,
                second: tm.tm_sec,
            };
            assert_eq!(civil_from_unix(ts).unwrap(), libc_civil, "vs gmtime_r({ts})");

            let mut tm2 = tm;
            let libc_secs = unsafe { libc::timegm(&mut tm2) } as i64;
            assert_eq!(unix_from_civil(libc_civil).unwrap(), libc_secs);
        }
    }
}
