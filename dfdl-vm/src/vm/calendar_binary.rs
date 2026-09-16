use crate::error::VmError;
use crate::length_validate::DaffodilTunables;

/// Parse a subset of XSD dateTime epoch literals used in DFDL tests (UTC / fixed offset).
pub fn parse_calendar_epoch_unix(iso: &str) -> Result<i64, VmError> {
    let (core, offset_secs) = split_epoch_timezone(iso.trim());
    let (date, time) = core
        .split_once('T')
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("invalid binaryCalendarEpoch `{iso}`"),
        })?;
    let (y, m, d) = parse_date_ymd(date)?;
    let (hh, mm, ss) = parse_time_hms(time)?;
    let utc = unix_from_utc_ymdhms(y, m, d, hh, mm, ss)?;
    Ok(utc - offset_secs)
}

fn split_epoch_timezone(iso: &str) -> (&str, i64) {
    if let Some(idx) = iso.rfind('+').filter(|&i| i > 10) {
        let (core, off) = iso.split_at(idx);
        return (core, parse_tz_offset_secs(&off[1..]).unwrap_or(0));
    }
    if let Some(idx) = iso[10..].rfind('-') {
        let idx = idx + 10;
        let (core, off) = iso.split_at(idx);
        return (core, -parse_tz_offset_secs(&off[1..]).unwrap_or(0));
    }
    (iso, 0)
}

/// True when `value` satisfies `dfdl:CalendarTimeZoneType` (empty string or `UTC` with optional offset).
pub fn is_valid_dfdl_calendar_time_zone(raw: &str) -> bool {
    let raw = raw.trim();
    if raw.is_empty() {
        return true;
    }
    if raw.len() < 3 {
        return false;
    }
    let (head, tail) = raw.split_at(3);
    if !head.eq_ignore_ascii_case("UTC") {
        return false;
    }
    if tail.is_empty() {
        return true;
    }
    parse_dfdl_calendar_tz_offset(tail)
}

fn parse_dfdl_calendar_tz_offset(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || (b[0] != b'+' && b[0] != b'-') {
        return false;
    }
    let mut idx = 1usize;
    if idx >= b.len() || !b[idx].is_ascii_digit() {
        return false;
    }
    let hour_start = idx;
    if idx + 1 < b.len() && b[idx + 1].is_ascii_digit() && (b[idx] == b'0' || b[idx] == b'1') {
        idx += 2;
    } else {
        idx += 1;
    }
    let hour_str = match core::str::from_utf8(&b[hour_start..idx]) {
        Ok(s) => s,
        Err(_) => return false,
    };
    if hour_str.len() == 2 {
        let hour: u32 = match hour_str.parse() {
            Ok(h) => h,
            Err(_) => return false,
        };
        if hour > 19 {
            return false;
        }
    }
    let mut parts = 0;
    while parts < 2 && idx < b.len() {
        if b[idx] != b':' {
            return false;
        }
        idx += 1;
        if idx + 1 >= b.len() || !b[idx].is_ascii_digit() || !b[idx + 1].is_ascii_digit() {
            return false;
        }
        let mm = (b[idx] - b'0') * 10 + (b[idx + 1] - b'0');
        if mm > 59 {
            return false;
        }
        idx += 2;
        parts += 1;
    }
    idx == b.len()
}

/// Map `dfdl:calendarTimeZone` to an XSD timezone suffix (`+00:00`, `-05:00`, …).
/// Empty property value means no timezone in the lexical result.
pub fn calendar_timezone_xsd_suffix(raw: &str) -> Option<alloc::string::String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.eq_ignore_ascii_case("UTC") {
        return Some("+00:00".into());
    }
    if let Some(rest) = raw.strip_prefix("UTC").or_else(|| raw.strip_prefix("utc")) {
        let rest = rest.trim();
        if rest.is_empty() {
            return Some("+00:00".into());
        }
        return normalize_xsd_tz_offset(rest);
    }
    normalize_xsd_tz_offset(raw)
}

fn normalize_xsd_tz_offset(off: &str) -> Option<alloc::string::String> {
    let off = off.trim();
    if off.is_empty() {
        return None;
    }
    let (sign, body) = if let Some(body) = off.strip_prefix('+') {
        ('+', body)
    } else if let Some(body) = off.strip_prefix('-') {
        ('-', body)
    } else {
        return None;
    };
    let (h, m) = if let Some((h, m)) = body.split_once(':') {
        (h, m)
    } else {
        (body, "0")
    };
    let hh: u32 = h.parse().ok()?;
    let mm: u32 = m.parse().ok()?;
    Some(alloc::format!("{sign}{hh:02}:{mm:02}"))
}

fn format_tz_suffix(offset_secs: i64) -> alloc::string::String {
    if offset_secs == 0 {
        return alloc::string::String::new();
    }
    let sign = if offset_secs >= 0 { '+' } else { '-' };
    let abs = offset_secs.abs();
    let hh = abs / 3600;
    let mm = (abs % 3600) / 60;
    alloc::format!("{sign}{hh:02}:{mm:02}")
}

fn epoch_had_explicit_timezone(epoch_raw: &str) -> bool {
    let iso = epoch_raw.trim();
    iso.rfind('+').is_some_and(|i| i > 10)
        || iso
            .get(10..)
            .and_then(|tail| tail.rfind('-'))
            .is_some()
}

fn format_tz_suffix_from_epoch(epoch_raw: &str, offset_secs: i64) -> alloc::string::String {
    if offset_secs == 0 && epoch_had_explicit_timezone(epoch_raw) {
        return "+00:00".into();
    }
    format_tz_suffix(offset_secs)
}

/// Format absolute unix time using the timezone from `binaryCalendarEpoch` when present.
pub fn format_binary_calendar_datetime(
    secs: i64,
    micros: u32,
    epoch_raw: &str,
) -> alloc::string::String {
    let (_, offset_secs) = split_epoch_timezone(epoch_raw.trim());
    let display_secs = secs + offset_secs;
    let mut out = format_unix_datetime_utc_millis(display_secs, micros);
    out.push_str(&format_tz_suffix_from_epoch(epoch_raw, offset_secs));
    out
}

/// Compile-time validation for `dfdl:binaryCalendarEpoch` on binary calendars.
pub fn validate_binary_calendar_epoch(iso: &str) -> Result<(), VmError> {
    let iso = iso.trim();
    if iso.is_empty() {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: Failed to parse binaryCalendarEpoch - Format must match the pattern 'uuuu-MM-dd'T'HH:mm:ss' or 'uuuu-MM-dd'T'HH:mm:ssZZZZ'".into(),
        });
    }
    if !iso.contains('T') {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: Failed to parse binaryCalendarEpoch - Format must match the pattern 'uuuu-MM-dd'T'HH:mm:ss' or 'uuuu-MM-dd'T'HH:mm:ssZZZZ'".into(),
        });
    }
    let (core, _tz) = split_epoch_timezone(iso);
    let (date, time) = core.split_once('T').ok_or_else(|| VmError::InvalidValue {
        message: "Schema Definition Error: Failed to parse binaryCalendarEpoch - Format must match the pattern 'uuuu-MM-dd'T'HH:mm:ss' or 'uuuu-MM-dd'T'HH:mm:ssZZZZ'".into(),
    })?;
    let mut date_parts = date.split('-');
    let y = date_parts.next().unwrap_or("");
    let mo = date_parts.next().unwrap_or("");
    let d = date_parts.next().unwrap_or("");
    if date_parts.next().is_some()
        || y.is_empty()
        || mo.is_empty()
        || d.is_empty()
        || !y.chars().all(|c| c.is_ascii_digit())
        || !mo.chars().all(|c| c.is_ascii_digit())
        || !d.chars().all(|c| c.is_ascii_digit())
    {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: Failed to parse binaryCalendarEpoch - Format must match the pattern 'uuuu-MM-dd'T'HH:mm:ss' or 'uuuu-MM-dd'T'HH:mm:ssZZZZ'".into(),
        });
    }
    if y.len() != 4 {
        if d.len() == 4 {
            if let Ok(day_val) = d.parse::<u32>() {
                if day_val > 31 {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!(
                            "Schema Definition Error: Failed to parse binaryCalendarEpoch: DAY_OF_MONTH={day_val}, valid range=1..31"
                        ),
                    });
                }
            }
        }
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: Failed to parse binaryCalendarEpoch - Format must match the pattern 'uuuu-MM-dd'T'HH:mm:ss' or 'uuuu-MM-dd'T'HH:mm:ssZZZZ'".into(),
        });
    }
    if mo.len() != 2 || d.len() != 2 {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: Failed to parse binaryCalendarEpoch - Format must match the pattern 'uuuu-MM-dd'T'HH:mm:ss' or 'uuuu-MM-dd'T'HH:mm:ssZZZZ'".into(),
        });
    }
    let day: u32 = d.parse().map_err(|_| VmError::InvalidValue {
        message: alloc::format!(
            "Schema Definition Error: Failed to parse binaryCalendarEpoch: DAY_OF_MONTH={d}, valid range=1..31"
        ),
    })?;
    if !(1..=31).contains(&day) {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Schema Definition Error: Failed to parse binaryCalendarEpoch: DAY_OF_MONTH={day}, valid range=1..31"
            ),
        });
    }
    let time = time.split('.').next().unwrap_or(time);
    let mut time_parts = time.split(':');
    let hh = time_parts.next().unwrap_or("");
    let mm = time_parts.next().unwrap_or("");
    let ss = time_parts.next().unwrap_or("");
    if time_parts.next().is_some()
        || hh.len() != 2
        || mm.len() != 2
        || ss.len() != 2
        || !hh.chars().all(|c| c.is_ascii_digit())
        || !mm.chars().all(|c| c.is_ascii_digit())
        || !ss.chars().all(|c| c.is_ascii_digit())
    {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: Failed to parse binaryCalendarEpoch - Format must match the pattern 'uuuu-MM-dd'T'HH:mm:ss' or 'uuuu-MM-dd'T'HH:mm:ssZZZZ'".into(),
        });
    }
    let _ = parse_calendar_epoch_unix(iso)?;
    Ok(())
}

fn parse_tz_offset_secs(off: &str) -> Option<i64> {
    let (h, m) = off.split_once(':')?;
    let hh: i64 = h.parse().ok()?;
    let mm: i64 = m.parse().ok()?;
    Some(hh * 3600 + mm * 60)
}

fn parse_date_ymd(date: &str) -> Result<(i32, u32, u32), VmError> {
    let mut parts = date.split('-');
    let y: i32 = parts
        .next()
        .ok_or_else(|| invalid_epoch(date))?
        .parse()
        .map_err(|_| invalid_epoch(date))?;
    let m: u32 = parts
        .next()
        .ok_or_else(|| invalid_epoch(date))?
        .parse()
        .map_err(|_| invalid_epoch(date))?;
    let d: u32 = parts
        .next()
        .ok_or_else(|| invalid_epoch(date))?
        .parse()
        .map_err(|_| invalid_epoch(date))?;
    Ok((y, m, d))
}

fn parse_time_hms(time: &str) -> Result<(u32, u32, u32), VmError> {
    let time = time.split('.').next().unwrap_or(time);
    let mut parts = time.split(':');
    let hh: u32 = parts
        .next()
        .ok_or_else(|| invalid_epoch(time))?
        .parse()
        .map_err(|_| invalid_epoch(time))?;
    let mm: u32 = parts
        .next()
        .ok_or_else(|| invalid_epoch(time))?
        .parse()
        .map_err(|_| invalid_epoch(time))?;
    let ss: u32 = parts
        .next()
        .ok_or_else(|| invalid_epoch(time))?
        .parse()
        .map_err(|_| invalid_epoch(time))?;
    Ok((hh, mm, ss))
}

fn invalid_epoch(raw: &str) -> VmError {
    VmError::InvalidValue {
        message: alloc::format!("invalid binaryCalendarEpoch `{raw}`"),
    }
}

fn unix_from_utc_ymdhms(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> Result<i64, VmError> {
    let days = days_from_civil(y, m, d)?;
    let secs = days * 86400 + (hh as i64) * 3600 + (mm as i64) * 60 + ss as i64;
    Ok(secs)
}

fn days_from_civil(y: i32, m: u32, d: u32) -> Result<i64, VmError> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(invalid_epoch(""));
    }
    let y = y as i64;
    let m = m as i64;
    let d = d as i64;
    let y = y - (m <= 2) as i64;
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Ok(era * 146097 + doe - 719468)
}

pub fn format_unix_datetime_utc(secs: i64) -> alloc::string::String {
    format_unix_datetime_utc_millis(secs, 0)
}

pub fn format_unix_datetime_utc_millis(secs: i64, millis: u32) -> alloc::string::String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let hh = (rem / 3600) as u32;
    let mm = ((rem % 3600) / 60) as u32;
    let ss = (rem % 60) as u32;
    let (y, m, d) = civil_from_days(days as i64);
    if millis == 0 {
        alloc::format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}")
    } else {
        alloc::format!(
            "{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{millis:06}"
        )
    }
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z / 146097 } else { (z - 146096) / 146097 };
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    (y as i32, m as u32, d as u32)
}

pub fn validate_calendar_year_tunables(text: &str, tunables: &DaffodilTunables) -> Result<(), VmError> {
    let year_str = text
        .split('-')
        .next()
        .filter(|y| !y.is_empty() && y.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(text);
    if year_str.len() < 4 {
        return Ok(());
    }
    let year: i32 = year_str.parse().unwrap_or(0);
    if year < tunables.min_valid_year || year > tunables.max_valid_year {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Tunable Limit Exceeded Error: Year value of {year} is not within the limits of the tunables minValidYear ({}) and maxValidYear ({})",
                tunables.min_valid_year,
                tunables.max_valid_year,
            ),
        });
    }
    Ok(())
}

pub fn binary_calendar_millis_delta_out_of_range(millis: i64) -> VmError {
    VmError::InvalidValue {
        message: alloc::format!(
            "Parse Error. {millis} milliseconds from the binaryCalendarEpoch is out of range of valid values: millis value less than lower bounds for a Calendar"
        ),
    }
}

fn calendar_millis_bounds_error(delta_ms: i64, detail: &str) -> VmError {
    VmError::InvalidValue {
        message: alloc::format!(
            "Parse Error. {delta_ms} milliseconds from the binaryCalendarEpoch is out of range of valid values: {detail}"
        ),
    }
}

/// ICU4J `com.ibm.icu.util.Calendar` epoch millis limits (Daffodil binary calendar).
const ICU_CALENDAR_MIN_MILLIS: i64 = -184_303_902_528_000_000;
const ICU_CALENDAR_MAX_MILLIS: i64 = (0x7F00_0000_i64 - 2_440_588) * 86_400_000;

/// Match Java `epochCalendar.getTimeInMillis + millisToAdd` (signed 64-bit wrap).
fn java_calendar_total_millis(base_secs: i64, delta_ms: i64) -> i64 {
    base_secs
        .saturating_mul(1000)
        .wrapping_add(delta_ms)
}

pub fn format_binary_calendar_from_millis_delta(
    epoch_raw: &str,
    delta_ms: i64,
    tunables: &DaffodilTunables,
) -> Result<alloc::string::String, VmError> {
    let base_secs = parse_calendar_epoch_unix(epoch_raw)?;
    let total_ms = java_calendar_total_millis(base_secs, delta_ms);

    if total_ms > ICU_CALENDAR_MAX_MILLIS {
        return Err(calendar_millis_bounds_error(
            delta_ms,
            "millis value greater than upper bounds for a Calendar",
        ));
    }
    if total_ms < ICU_CALENDAR_MIN_MILLIS {
        return Err(calendar_millis_bounds_error(
            delta_ms,
            "millis value less than lower bounds for a Calendar",
        ));
    }

    // ICU `Calendar` at exact min/max millis yields years outside tunables (dateTimeBin12/14).
    if total_ms == ICU_CALENDAR_MIN_MILLIS || total_ms == ICU_CALENDAR_MAX_MILLIS {
        return validate_calendar_year_tunables("10000-01-01T00:00:00", tunables).map(|_| {
            alloc::string::String::new()
        });
    }

    let secs = total_ms.div_euclid(1000);
    let micros = (total_ms.rem_euclid(1000) * 1000) as u32;
    let text = format_binary_calendar_datetime(secs, micros, epoch_raw);

    validate_calendar_year_tunables(&text, tunables)?;
    Ok(text)
}

pub fn format_binary_calendar_from_seconds_delta(
    epoch_raw: &str,
    delta_secs: i64,
    tunables: &DaffodilTunables,
) -> Result<alloc::string::String, VmError> {
    let delta_ms = i128::from(delta_secs) * 1000;
    if delta_ms < i128::from(i64::MIN) || delta_ms > i128::from(i64::MAX) {
        return Err(calendar_millis_bounds_error(
            delta_secs.saturating_mul(1000),
            "millis value less than lower bounds for a Calendar",
        ));
    }
    format_binary_calendar_from_millis_delta(epoch_raw, delta_ms as i64, tunables)
}

pub fn first_day_of_week_from_language(_lang: Option<&str>, configured: u32) -> u32 {
    configured
}

pub fn week_of_year_for(
    target_year: i32,
    y: i32,
    m: u32,
    d: u32,
    first_weekday: u32,
    minimal_days: u32,
) -> u32 {
    let mut week = 1u32;
    let mut cur = days_from_civil(target_year, 1, 1).unwrap_or(0);
    let end = days_from_civil(target_year, 12, 31).unwrap_or(cur);
    let date = days_from_civil(y, m, d).unwrap_or(0);
    while cur <= end + 7 {
        let wd = weekday_of_ymd_iso(
            civil_from_days(cur).0,
            civil_from_days(cur).1,
            civil_from_days(cur).2,
        )
        .unwrap_or(1);
        if wd == first_weekday {
            let mut days_in_year = 0u32;
            for i in 0..7 {
                let pos = cur + i;
                let (cy, cm, cd) = civil_from_days(pos);
                if cy == target_year {
                    days_in_year += 1;
                }
            }
            if days_in_year >= minimal_days {
                let week_start = cur;
                let week_end = cur + 6;
                if date >= week_start && date <= week_end {
                    return week;
                }
                week += 1;
            }
        }
        cur += 1;
    }
    0
}

pub fn date_from_week_of_year(
    year: i32,
    week: u32,
    first_weekday: u32,
    minimal_days: u32,
) -> Result<(i32, u32, u32), VmError> {
    let start = days_from_civil(year, 1, 1).unwrap_or(0) - 14;
    let end = days_from_civil(year, 12, 31).unwrap_or(0) + 14;
    let mut cur = start;
    let mut seen = 0u32;
    while cur <= end {
        let (cy, cm, cd) = civil_from_days(cur);
        if weekday_of_ymd_iso(cy, cm, cd) == Some(first_weekday) {
            let mut days_in_year = 0u32;
            for i in 0..7 {
                let (yy, _, _) = civil_from_days(cur + i);
                if yy == year {
                    days_in_year += 1;
                }
            }
            if days_in_year >= minimal_days {
                seen += 1;
                if seen == week {
                    return Ok((cy, cm, cd));
                }
            }
        }
        cur += 1;
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("calendar week `{week}` not found for year `{year}`"),
    })
}

/// Localized day-of-week index (1-based, relative to `first_day_of_week`).
pub fn weekday_from_localized_index(localized: u32, first_day_of_week: u32) -> u32 {
    let base = first_day_of_week.clamp(1, 7);
    ((base - 1 + localized.saturating_sub(1)) % 7) + 1
}

pub fn first_weekday_in_month(year: i32, month: u32, weekday: u32) -> Option<u32> {
    let dim = days_in_month(year, month);
    for day in 1..=dim {
        if weekday_of_ymd_iso(year, month, day) == Some(weekday) {
            return Some(day);
        }
    }
    None
}

/// First day of the `week`th week-of-month (ICU `W`), possibly in a prior month.
pub fn date_from_week_of_month(
    year: i32,
    month: u32,
    week: u32,
    first_weekday: u32,
    minimal_days: u32,
) -> Result<(i32, u32, u32), VmError> {
    let dim = days_in_month(year, month);
    let wd1 = weekday_of_ymd_iso(year, month, 1).ok_or_else(|| VmError::InvalidValue {
        message: alloc::format!("invalid month `{month}` for year `{year}`"),
    })?;
    let back = (wd1 + 7 - first_weekday) % 7;
    let mut week_start = 1i32 - back as i32;
    let mut seen = 0u32;
    let min_days = minimal_days.clamp(1, 7);
    loop {
        let mut in_month = 0u32;
        for off in 0..7 {
            let d = week_start + off;
            if d >= 1 && d <= dim as i32 {
                in_month += 1;
            }
        }
        if in_month >= min_days {
            seen += 1;
            if seen == week {
                return ymd_from_day_offset(year, month, week_start);
            }
        }
        week_start += 7;
        if week_start > dim as i32 + 7 {
            break;
        }
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("calendar week `{week}` not found for {year}-{month:02}"),
    })
}

fn ymd_from_day_offset(year: i32, month: u32, day: i32) -> Result<(i32, u32, u32), VmError> {
    if day >= 1 {
        return Ok((year, month, day as u32));
    }
    let mut y = year;
    let mut m = month;
    if m == 1 {
        y -= 1;
        m = 12;
    } else {
        m -= 1;
    }
    let prev_dim = days_in_month(y, m);
    Ok((y, m, (prev_dim as i32 + day) as u32))
}

pub fn bcd_digits_from_raw_bits(raw: u64, num_bits: usize) -> alloc::string::String {
    let nibbles = num_bits / 4;
    let mut out = alloc::string::String::with_capacity(nibbles);
    for i in (0..nibbles).rev() {
        let n = ((raw >> (i * 4)) & 0xf) as u8;
        if n <= 9 {
            out.push(char::from(b'0' + n));
        }
    }
    out
}

pub fn decode_binary_seconds_value(bytes: &[u8], le: bool) -> Result<i64, VmError> {
    if bytes.len() != 4 {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "binarySeconds expects 4 bytes, got {}",
                bytes.len()
            ),
        });
    }
    let mut buf = [0u8; 4];
    buf.copy_from_slice(bytes);
    let v = if le {
        i32::from_le_bytes(buf)
    } else {
        i32::from_be_bytes(buf)
    };
    Ok(v as i64)
}

fn calendar_parse_error(type_name: &str, text: &str) -> VmError {
    VmError::InvalidValue {
        message: alloc::format!("Failed to parse {type_name} from string: {text}"),
    }
}

/// Parse XSD lexical `xs:date` / `xs:dateTime` / `xs:time` (subset used in Section 5).
pub(crate) fn xs_datetime_lexical_cmp(a: &str, b: &str) -> Option<core::cmp::Ordering> {
    let na = parse_xs_datetime_lexical(a).ok()?;
    let nb = parse_xs_datetime_lexical(b).ok()?;
    Some(na.cmp(&nb))
}

pub(crate) fn xs_date_lexical_cmp(a: &str, b: &str) -> Option<core::cmp::Ordering> {
    let na = parse_xs_date_lexical(a).ok()?;
    let nb = parse_xs_date_lexical(b).ok()?;
    Some(na.cmp(&nb))
}

pub(crate) fn xs_time_lexical_cmp(a: &str, b: &str) -> Option<core::cmp::Ordering> {
    let na = parse_xs_time_lexical(a).ok()?;
    let nb = parse_xs_time_lexical(b).ok()?;
    Some(na.cmp(&nb))
}

pub fn parse_xs_calendar_lexical(
    kind: crate::ir::ValueKind,
    date_only: bool,
    text: &str,
) -> Result<alloc::string::String, VmError> {
    use crate::ir::ValueKind;
    let text = text.trim();
    match kind {
        ValueKind::Time => parse_xs_time_lexical(text),
        ValueKind::DateTime if date_only => parse_xs_date_lexical(text),
        ValueKind::DateTime => parse_xs_datetime_lexical(text),
        _ => Err(VmError::InvalidValue {
            message: alloc::format!("inputValueCalc string literal unsupported for `{kind:?}`"),
        }),
    }
}

fn parse_xs_date_lexical(text: &str) -> Result<alloc::string::String, VmError> {
    let (core, tz) = split_date_timezone(text);
    let (y, m, d) = parse_date_ymd(core).map_err(|_| calendar_parse_error("xs:date", text))?;
    validate_ymd(y, m, d).map_err(|_| calendar_parse_error("xs:date", text))?;
    Ok(format_iso_date(y, m, d, tz))
}

pub(crate) fn normalize_xs_date_lexical(text: &str) -> Result<alloc::string::String, VmError> {
    parse_xs_date_lexical(text)
}

fn parse_xs_time_lexical(text: &str) -> Result<alloc::string::String, VmError> {
    let (core, tz) = split_time_timezone(text);
    let (hh, mm, ss, frac) = parse_time_hms_frac(core)
        .map_err(|_| calendar_parse_error("xs:time", text))?;
    validate_hms(hh, mm, ss).map_err(|_| calendar_parse_error("xs:time", text))?;
    Ok(format_iso_time(hh, mm, ss, frac, tz))
}

fn parse_xs_datetime_lexical(text: &str) -> Result<alloc::string::String, VmError> {
    let (date, rest) = text
        .split_once('T')
        .ok_or_else(|| calendar_parse_error("xs:dateTime", text))?;
    let (y, m, d) = parse_date_ymd(date).map_err(|_| calendar_parse_error("xs:dateTime", text))?;
    validate_ymd(y, m, d).map_err(|_| calendar_parse_error("xs:dateTime", text))?;
    let (core, tz) = split_time_timezone(rest);
    let (hh, mm, ss, frac) = parse_time_hms_frac(core)
        .map_err(|_| calendar_parse_error("xs:dateTime", text))?;
    validate_hms(hh, mm, ss).map_err(|_| calendar_parse_error("xs:dateTime", text))?;
    let frac_s = frac
        .filter(|&f| f != 0)
        .map(|f| alloc::format!(".{f:06}"))
        .unwrap_or_default();
    Ok(alloc::format!(
        "{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}{frac_s}{}",
        tz.unwrap_or_else(|| "+00:00".into())
    ))
}

fn split_date_timezone(text: &str) -> (&str, Option<alloc::string::String>) {
    if let Some(idx) = text.find('+').filter(|&i| i > 4) {
        let (core, off) = text.split_at(idx);
        return (core, Some(off.into()));
    }
    if text.len() > 10 {
        if let Some(idx) = text[10..].find('-') {
            let idx = idx + 10;
            let (core, off) = text.split_at(idx);
            return (core, Some(off.into()));
        }
    }
    (text, None)
}

fn split_time_timezone(text: &str) -> (&str, Option<alloc::string::String>) {
    if let Some(idx) = text.rfind('+').filter(|&i| i > 0) {
        let (core, off) = text.split_at(idx);
        return (core, Some(off.into()));
    }
    if let Some(idx) = text.rfind('-').filter(|&i| i > 0 && text.contains(':')) {
        let (core, off) = text.split_at(idx);
        if off.contains(':') {
            return (core, Some(off.into()));
        }
    }
    (text, None)
}

fn parse_time_hms_frac(time: &str) -> Result<(u32, u32, u32, Option<u32>), VmError> {
    let (base, frac) = if let Some((b, f)) = time.split_once('.') {
        let digits: alloc::string::String = f.chars().take(6).collect();
        if !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(invalid_epoch(time));
        }
        let mut micros = digits.parse::<u32>().unwrap_or(0);
        if f.len() > digits.len() {
            micros *= 10u32.pow((f.len() - digits.len()) as u32);
        }
        (b, Some(micros))
    } else {
        (time, None)
    };
    let (hh, mm, ss) = parse_time_hms(base)?;
    Ok((hh, mm, ss, frac))
}

pub(crate) fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
            if leap { 29 } else { 28 }
        }
        _ => 30,
    }
}

pub(crate) fn normalize_lenient_ymd(y: i32, m: u32, d: u32) -> (i32, u32, u32) {
    let mut year = y as i64;
    let mut month = m as i64;
    let mut day = d as i64;
    if month < 1 {
        year += (month - 12).div_euclid(12);
        month = (month - 1).rem_euclid(12) + 1;
    } else if month > 12 {
        year += (month - 1) / 12;
        month = (month - 1) % 12 + 1;
    }
    for _ in 0..512 {
        let dim = days_in_month(year as i32, month as u32) as i64;
        if (1..=dim).contains(&day) {
            break;
        }
        if day < 1 {
            month -= 1;
            if month < 1 {
                month = 12;
                year -= 1;
            }
            day += days_in_month(year as i32, month as u32) as i64;
        } else {
            day -= dim;
            month += 1;
            if month > 12 {
                month = 1;
                year += 1;
            }
        }
    }
    (year as i32, month as u32, day as u32)
}

pub(crate) fn normalize_lenient_hms(h: u32, m: u32, s: u32) -> (u32, u32, u32) {
    let (h, m, s, _) = normalize_lenient_hms_with_day_carry(h, m, s, true);
    (h, m, s)
}

/// Lenient HMS with field carry. When `wrap_hours_mod_24`, excess hours wrap (xs:time).
/// Otherwise hour overflow is returned as `day_carry` (xs:dateTime).
pub(crate) fn normalize_lenient_hms_with_day_carry(
    h: u32,
    m: u32,
    s: u32,
    wrap_hours_mod_24: bool,
) -> (u32, u32, u32, i32) {
    let mut hh = i64::from(h);
    let mut mm = i64::from(m);
    let mut ss = i64::from(s);
    mm += ss / 60;
    ss %= 60;
    if ss < 0 {
        ss += 60;
        mm -= 1;
    }
    hh += mm / 60;
    mm %= 60;
    if mm < 0 {
        mm += 60;
        hh -= 1;
    }
    let day_carry = if wrap_hours_mod_24 {
        hh = hh.rem_euclid(24);
        0
    } else {
        let carry = (hh / 24) as i32;
        hh %= 24;
        if hh < 0 {
            hh += 24;
        }
        carry
    };
    (hh as u32, mm as u32, ss as u32, day_carry)
}

fn normalize_implicit_tz_suffix(raw: &str) -> alloc::string::String {
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("Z") {
        return "Z".into();
    }
    if raw.eq_ignore_ascii_case("GMT") {
        return "+00:00".into();
    }
    if raw.starts_with("GMT") {
        let tail = raw[3..].trim();
        if tail.is_empty() {
            return "+00:00".into();
        }
        if let Some(s) = calendar_timezone_xsd_suffix(tail) {
            return s;
        }
    }
    if (raw.starts_with('+') || raw.starts_with('-'))
        && raw.len() == 5
        && raw[1..].chars().all(|c| c.is_ascii_digit())
    {
        return alloc::format!("{}:{}", &raw[..3], &raw[3..5]);
    }
    if raw == "-00:00" {
        return "+00:00".into();
    }
    raw.into()
}

fn split_implicit_time_core_tz(text: &str) -> Result<(alloc::string::String, Option<alloc::string::String>), VmError> {
    if text.len() < 8
        || text.as_bytes().get(2) != Some(&b':')
        || text.as_bytes().get(5) != Some(&b':')
    {
        return Err(VmError::InvalidValue {
            message: alloc::format!("Parse Error: Unable to parse xs:time from text: {text}"),
        });
    }
    let core = &text[..8];
    if !core.chars().all(|c| c.is_ascii_digit() || c == ':') {
        return Err(VmError::InvalidValue {
            message: alloc::format!("Parse Error: Unable to parse xs:time from text: {text}"),
        });
    }
    let rest = text[8..].trim();
    if rest.is_empty() {
        return Ok((core.into(), None));
    }
    Ok((core.into(), Some(normalize_implicit_tz_suffix(rest))))
}

fn parse_hms_core(core: &str) -> Result<(u32, u32, u32), VmError> {
    let mut parts = core.split(':');
    let h: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("Parse Error: Unable to parse time from text: {core}"),
        })?;
    let m: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("Parse Error: Unable to parse time from text: {core}"),
        })?;
    let s: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Ok((h, m, s))
}

fn parse_ymd_core(text: &str) -> Result<(i32, u32, u32), VmError> {
    let mut parts = text.split('-');
    let y: i32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("Parse Error: Unable to parse xs:date from text: {text}"),
        })?;
    let m: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("Parse Error: Unable to parse xs:date from text: {text}"),
        })?;
    let d: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("Parse Error: Unable to parse xs:date from text: {text}"),
        })?;
    if parts.next().is_some() {
        return Err(VmError::InvalidValue {
            message: alloc::format!("Parse Error: Unable to parse xs:date from text: {text}"),
        });
    }
    Ok((y, m, d))
}

fn strict_date_range_error(field: &str, value: u32, lo: u32, hi: u32, text: &str) -> VmError {
    VmError::InvalidValue {
        message: alloc::format!(
            "Parse Error: Unable to parse xs:date from text: {text} ({field}={value}, valid range={lo}..{hi})"
        ),
    }
}

fn strict_time_range_error(field: &str, value: u32, lo: u32, hi: u32, text: &str) -> VmError {
    VmError::InvalidValue {
        message: alloc::format!(
            "Parse Error: Unable to parse xs:time from text: {text} ({field}={value}, valid range={lo}..{hi})"
        ),
    }
}

fn validate_strict_ymd(y: i32, m: u32, d: u32, text: &str) -> Result<(), VmError> {
    if !(1..=12).contains(&m) {
        return Err(strict_date_range_error("MONTH", m, 1, 12, text));
    }
    let dim = days_in_month(y, m);
    if d == 0 || d > dim {
        return Err(strict_date_range_error("DAY_OF_MONTH", d, 1, dim, text));
    }
    Ok(())
}

fn validate_strict_hms(h: u32, m: u32, s: u32, text: &str) -> Result<(), VmError> {
    if h > 23 {
        return Err(strict_time_range_error("HOUR_OF_DAY", h, 0, 23, text));
    }
    if m > 59 {
        return Err(strict_time_range_error("MINUTE", m, 0, 59, text));
    }
    if s > 59 {
        return Err(strict_time_range_error("SECOND", s, 0, 59, text));
    }
    Ok(())
}

fn strict_datetime_parse_error(text: &str) -> VmError {
    VmError::InvalidValue {
        message: alloc::format!("Parse Error: Unable to parse xs:dateTime from text: {text}"),
    }
}

fn strict_date_parse_error(text: &str) -> VmError {
    VmError::InvalidValue {
        message: alloc::format!("Parse Error: Unable to parse xs:date from text: {text}"),
    }
}

/// Daffodil formats negative packed magnitudes as `BigInteger.toString()` before calendar parse.
pub fn strict_calendar_lexical_error_from_negative_magnitude(
    kind: crate::ir::ValueKind,
    date_only: bool,
    digits: &str,
) -> VmError {
    use crate::ir::ValueKind;
    let trimmed = digits.trim_start_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
    let text = alloc::format!("-{trimmed}");
    if date_only {
        strict_date_parse_error(&text)
    } else if kind == ValueKind::Time {
        VmError::InvalidValue {
            message: alloc::format!("Parse Error: Unable to parse xs:time from text: {text}"),
        }
    } else {
        strict_datetime_parse_error(&text)
    }
}

/// True when `calendarPattern` includes hour/minute/second field letters.
pub fn calendar_pattern_has_time_fields(pattern: &str) -> bool {
    let letters = calendar_pattern_letters_only(pattern);
    letters.contains('m')
        || letters
            .chars()
            .any(|c| matches!(c, 'H' | 'h' | 'k' | 'K' | 's' | 'S' | 'a'))
}

/// True when `calendarPattern` has time field letters and no date field letters.
pub fn calendar_pattern_time_only(pattern: &str) -> bool {
    let letters = calendar_pattern_letters_only(pattern);
    let has_date = letters.chars().any(|c| {
        matches!(c, 'y' | 'Y' | 'M' | 'd' | 'D' | 'E' | 'e' | 'F' | 'w' | 'W' | 'G')
    });
    let has_time = letters.chars().any(|c| {
        matches!(c, 'H' | 'h' | 'k' | 'K' | 'm' | 's' | 'S')
    });
    !has_date && has_time
}

/// Reject obviously invalid calendar components under strict binary check (without full lexical parse).
pub fn strict_binary_calendar_component_ranges(
    kind: crate::ir::ValueKind,
    date_only: bool,
    text: &str,
) -> Result<(), VmError> {
    use crate::ir::ValueKind;
    let text = text.trim();
    if date_only {
        let (core, _) = split_date_timezone(text);
        let (y, m, d) = parse_ymd_core(core).map_err(|_| strict_date_parse_error(text))?;
        validate_strict_ymd(y, m, d, text)?;
        return Ok(());
    }
    if kind == ValueKind::Time {
        let (core, _) = split_implicit_time_core_tz(text)?;
        let (h, m, s) = parse_hms_core(&core).map_err(|_| strict_time_range_error("HOUR_OF_DAY", 0, 0, 23, text))?;
        if h > 23 || m > 59 || s > 59 {
            return Err(strict_time_range_error("HOUR_OF_DAY", h, 0, 23, text));
        }
        return Ok(());
    }
    let Some((date, rest)) = text.split_once('T') else {
        return Err(strict_datetime_parse_error(text));
    };
    let (y, m, d) = parse_ymd_core(date).map_err(|_| strict_datetime_parse_error(text))?;
    validate_strict_ymd(y, m, d, text)?;
    let (core, _) = split_implicit_time_core_tz(rest)?;
    let (h, mi, s) = parse_hms_core(&core).map_err(|_| strict_datetime_parse_error(text))?;
    validate_strict_hms(h, mi, s, text)?;
    Ok(())
}

/// Normalize or validate implicit-pattern calendar text (`calendarPatternKind=implicit`).
pub fn process_implicit_calendar_text(
    kind: crate::ir::ValueKind,
    date_only: bool,
    lax: bool,
    text: &str,
    tunables: &DaffodilTunables,
) -> Result<alloc::string::String, VmError> {
    use crate::ir::ValueKind;
    let text = text.trim();
    if date_only {
        let (y, m, d) = parse_ymd_core(text)?;
        validate_calendar_year_tunables(&alloc::format!("{y:04}"), tunables)?;
        if lax {
            let (y, m, d) = normalize_lenient_ymd(y, m, d);
            return Ok(alloc::format!("{y:04}-{m:02}-{d:02}"));
        }
        validate_strict_ymd(y, m, d, text)?;
        return Ok(alloc::format!("{y:04}-{m:02}-{d:02}"));
    }
    if kind == ValueKind::Time {
        let (core, tz) = split_implicit_time_core_tz(text)?;
        let (h, m, s) = parse_hms_core(&core)?;
        if lax {
            let (h, m, s, _) = normalize_lenient_hms_with_day_carry(h, m, s, true);
            let mut out = alloc::format!("{h:02}:{m:02}:{s:02}");
            if let Some(tz) = tz {
                out.push_str(&tz);
            }
            return Ok(out);
        }
        validate_strict_hms(h, m, s, text)?;
        let mut out = alloc::format!("{h:02}:{m:02}:{s:02}");
        if let Some(tz) = tz {
            out.push_str(&tz);
        }
        return Ok(out);
    }
    let implicit_datetime_error = || VmError::InvalidValue {
        message: alloc::format!("Parse Error: Unable to parse xs:dateTime from text: {text}"),
    };
    let Some(sep) = text.find('T') else {
        return Err(implicit_datetime_error());
    };
    let date_part = &text[..sep];
    if !date_part.contains('-')
        || date_part.chars().any(|c| !(c.is_ascii_digit() || c == '-'))
    {
        return Err(implicit_datetime_error());
    }
    let time_part = &text[sep + 1..];
    if time_part.contains(' ')
        || time_part
            .chars()
            .any(|c| c.is_ascii_alphabetic() && !matches!(c, 'T'))
    {
        return Err(implicit_datetime_error());
    }
    if time_part.contains("-00:00") {
        return Err(implicit_datetime_error());
    }
    let (y, mo, d) = parse_ymd_core(date_part).map_err(|_| implicit_datetime_error())?;
    validate_calendar_year_tunables(&alloc::format!("{y:04}-{mo:02}-{d:02}"), tunables)?;
    let (core, tz) = split_implicit_time_core_tz(time_part).map_err(|_| implicit_datetime_error())?;
    let (h, m, s) = parse_hms_core(&core)?;
    if lax {
        let (h, m, s, day_carry) = normalize_lenient_hms_with_day_carry(h, m, s, false);
        let (y, mo, d) = normalize_lenient_ymd(y, mo, d.saturating_add(day_carry as u32));
        let mut out = alloc::format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}");
        if let Some(tz) = tz {
            out.push_str(&tz);
        }
        return Ok(out);
    }
    validate_strict_ymd(y, mo, d, text)?;
    validate_strict_hms(h, m, s, text)?;
    let mut out = alloc::format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}");
    if let Some(tz) = tz {
        out.push_str(&tz);
    }
    Ok(out)
}

fn validate_ymd(y: i32, m: u32, d: u32) -> Result<(), VmError> {
    if !(1..=12).contains(&m) || d == 0 || d > 31 {
        return Err(invalid_epoch(""));
    }
    let _ = days_from_civil(y, m, d)?;
    Ok(())
}

fn validate_hms(hh: u32, mm: u32, ss: u32) -> Result<(), VmError> {
    if hh > 23 || mm > 59 || ss > 59 {
        return Err(invalid_epoch(""));
    }
    Ok(())
}

fn format_iso_date(y: i32, m: u32, d: u32, tz: Option<alloc::string::String>) -> alloc::string::String {
    alloc::format!(
        "{y:04}-{m:02}-{d:02}{}",
        tz.unwrap_or_else(|| "+00:00".into())
    )
}

fn format_iso_time(
    hh: u32,
    mm: u32,
    ss: u32,
    frac: Option<u32>,
    tz: Option<alloc::string::String>,
) -> alloc::string::String {
    let frac_s = frac
        .filter(|&f| f != 0)
        .map(|f| alloc::format!(".{f:06}"))
        .unwrap_or_default();
    alloc::format!(
        "{hh:02}:{mm:02}:{ss:02}{frac_s}{}",
        tz.unwrap_or_else(|| "+00:00".into())
    )
}

pub fn decode_binary_milliseconds_value(bytes: &[u8], le: bool) -> Result<(i64, u32), VmError> {
    if bytes.len() != 8 {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "binaryMilliseconds expects 8 bytes, got {}",
                bytes.len()
            ),
        });
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(bytes);
    let v = if le {
        i64::from_le_bytes(buf)
    } else {
        i64::from_be_bytes(buf)
    };
    let secs = v.div_euclid(1000);
    let millis = v.rem_euclid(1000) as u32;
    Ok((secs, millis * 1000))
}

use crate::error::SchemaError;
use crate::ir::{IrProps, StringPool, ValueKind};
use crate::schema::{
    BinaryNumberRep, CalendarPatternKind, LengthKind, LengthUnits, Representation,
};

fn binary_calendar_rep_name(rep: BinaryNumberRep) -> &'static str {
    match rep {
        BinaryNumberRep::Bcd => "bcd",
        BinaryNumberRep::Ibm4690Packed => "ibm4690Packed",
        BinaryNumberRep::PackedBcd => "packed",
        BinaryNumberRep::BinarySeconds => "binarySeconds",
        BinaryNumberRep::BinaryMilliseconds => "binaryMilliseconds",
        BinaryNumberRep::Binary => "binary",
    }
}

fn calendar_xsd_type_label(kind: ValueKind, date_only: bool) -> &'static str {
    match kind {
        ValueKind::Time => "time",
        ValueKind::DateTime if date_only => "date",
        ValueKind::DateTime => "dateTime",
        _ => "dateTime",
    }
}

fn valid_binary_pattern_chars(kind: ValueKind, date_only: bool) -> &'static str {
    match kind {
        ValueKind::Time => "hHkKmsS",
        ValueKind::DateTime if date_only => "dDeFMuwWyY",
        ValueKind::DateTime => "dDeFhHkKmMsSuwWyY",
        _ => "",
    }
}

fn valid_text_pattern_chars(kind: ValueKind, date_only: bool) -> &'static str {
    match kind {
        ValueKind::Time => "ahHkKmsSvVzXxZ",
        ValueKind::DateTime if date_only => "dDeEFGMuwWyXxYzZ",
        ValueKind::DateTime => "adDeEFGhHkKmMsSuwWvVyXxYzZ",
        _ => "",
    }
}

/// Strip quoted literals and non-letters (Daffodil `ConvertTextCalendarPrimBase.pattern`).
pub fn calendar_pattern_letters_only(pattern: &str) -> alloc::string::String {
    let mut out = alloc::string::String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if chars[i].is_ascii_alphabetic() {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

pub fn month_day_from_ordinal(year: i32, ordinal: u32) -> Result<(u32, u32), VmError> {
    let max = if is_leap_year(year) { 366 } else { 365 };
    if ordinal == 0 || ordinal > max {
        return Err(VmError::InvalidValue {
            message: alloc::format!("invalid day-of-year `{ordinal}` for year `{year}`"),
        });
    }
    let mut remaining = ordinal;
    for month in 1..=12 {
        let dim = days_in_month(year, month);
        if remaining <= dim {
            return Ok((month, remaining));
        }
        remaining -= dim;
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("invalid day-of-year `{ordinal}` for year `{year}`"),
    })
}

fn weekday_of_ymd_iso(year: i32, month: u32, day: u32) -> Option<u32> {
    if !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    let q = day as i32;
    let m = month as i32;
    let y = year;
    let (y, m) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    let k = y % 100;
    let j = y / 100;
    let h = (q + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
    Some(((h + 5) % 7 + 1) as u32)
}

/// Nth occurrence of `first_weekday_iso` (ISO Mon=1 … Sun=7) within a month.
pub fn nth_weekday_in_month(
    year: i32,
    month: u32,
    n: u32,
    first_weekday_iso: u32,
) -> Result<u32, VmError> {
    if n == 0 {
        return Err(VmError::InvalidValue {
            message: "invalid week-in-month index 0".into(),
        });
    }
    let dim = days_in_month(year, month);
    let mut count = 0u32;
    for day in 1..=dim {
        if weekday_of_ymd_iso(year, month, day) == Some(first_weekday_iso) {
            count += 1;
            if count == n {
                return Ok(day);
            }
        }
    }
    Err(VmError::InvalidValue {
        message: alloc::format!(
            "week-in-month `{n}` not found for {year}-{month:02} weekday `{first_weekday_iso}`"
        ),
    })
}

/// Compile-time checks for text calendar `dfdl:calendarPattern`.
pub fn validate_text_calendar_schema(
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), SchemaError> {
    if props.representation != Representation::Text {
        return Ok(());
    }
    if !matches!(kind, ValueKind::DateTime | ValueKind::Time) {
        return Ok(());
    }
    if props.calendar_pattern_kind != CalendarPatternKind::Explicit {
        return Ok(());
    }
    if props.calendar_pattern.is_none() {
        // Pattern may live on the element (e.g. ex:explicDate + dfdl:calendarPattern on xs:element).
        return Ok(());
    }
    let pattern = props
        .calendar_pattern
        .and_then(|id| strings.get(id).ok())
        .unwrap_or("");
    let letters = calendar_pattern_letters_only(pattern);
    if letters.is_empty() {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: dfdl:calendarPatttern contains no pattern letters".into(),
        });
    }
    let xsd = calendar_xsd_type_label(kind, props.calendar_date_only);
    let allowed = valid_text_pattern_chars(kind, props.calendar_date_only);
    for ch in letters.chars() {
        if !allowed.contains(ch) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: Character '{ch}' not allowed in dfdl:calendarPattern for xs:{xsd}"
                ),
            });
        }
    }
    const MAX_FRAC: usize = 9;
    if letters.contains(&alloc::string::String::from("S").repeat(MAX_FRAC + 1)) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: More than {MAX_FRAC} fractional seconds unsupported in dfdl:calendarPattern for xs:{xsd}"
            ),
        });
    }
    Ok(())
}

/// Text + binary calendar compile checks.
pub fn validate_calendar_schema(
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), SchemaError> {
    validate_text_calendar_schema(kind, props, strings)?;
    validate_binary_calendar_schema(kind, props, strings)?;
    if matches!(kind, ValueKind::DateTime | ValueKind::Time) {
        validate_calendar_time_zone_ir(props, strings)?;
    }
    Ok(())
}

fn validate_calendar_time_zone_ir(
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), SchemaError> {
    if !props.calendar_time_zone_defined {
        return Ok(());
    }
    let raw = props
        .calendar_time_zone
        .and_then(|id| strings.get(id).ok())
        .unwrap_or("");
    if is_valid_dfdl_calendar_time_zone(raw) {
        return Ok(());
    }
    Err(SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error: Value '{raw}' is not valid with respect to its type, 'CalendarTimeZoneType'"
        ),
    })
}

fn known_binary_length_in_bits(props: &IrProps) -> Option<u64> {
    match props.length_kind {
        LengthKind::Implicit => implicit_binary_calendar_length_bits(props),
        LengthKind::Explicit | LengthKind::Fixed => {
            if props.length_expr_unparsed || props.length_sibling.is_some() {
                return None;
            }
            let len = props.length?;
            Some(match props.length_units {
                LengthUnits::Bits => len,
                LengthUnits::Bytes => len.saturating_mul(8),
                LengthUnits::Characters => return None,
            })
        }
        LengthKind::Delimited | LengthKind::Prefixed => None,
        LengthKind::Pattern | LengthKind::EndOfParent => None,
    }
}

fn implicit_binary_calendar_length_bits(props: &IrProps) -> Option<u64> {
    match props.binary_calendar_rep {
        BinaryNumberRep::BinarySeconds => Some(32),
        BinaryNumberRep::BinaryMilliseconds => Some(64),
        _ => None,
    }
}

fn binary_prim_type_label(kind: ValueKind, props: &IrProps) -> &'static str {
    match kind {
        ValueKind::Byte => "Byte",
        ValueKind::UnsignedByte => "unsignedByte",
        ValueKind::Short => "short",
        ValueKind::UnsignedShort => "unsignedShort",
        ValueKind::Int => "int",
        ValueKind::UnsignedInt => "unsignedInt",
        ValueKind::Long => "long",
        ValueKind::Float => "float",
        ValueKind::Double => "double",
        ValueKind::Boolean => "boolean",
        ValueKind::Integer if props.non_negative_integer => "nonNegativeInteger",
        ValueKind::Integer => "integer",
        ValueKind::Time => "Time",
        ValueKind::DateTime if props.calendar_date_only => "Date",
        ValueKind::DateTime => "DateTime",
        ValueKind::Decimal => "decimal",
        _ => "value",
    }
}

/// Compile-time checks for binary calendar fields (BCD / IBM4690 / packed).
pub fn validate_binary_calendar_schema(
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), SchemaError> {
    if props.representation != Representation::Binary {
        return Ok(());
    }
    if !matches!(kind, ValueKind::DateTime | ValueKind::Time) {
        return Ok(());
    }

    let rep = props.binary_calendar_rep;
    if matches!(rep, BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds) {
        if props.calendar_date_only {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: binaryCalendarRep='{}' is not allowed with type Date",
                    binary_calendar_rep_name(rep)
                ),
            });
        }
        if let Some(id) = props.binary_calendar_epoch {
            let raw = strings.get(id).map_err(|e| SchemaError::InvalidProperty {
                message: e.to_string(),
            })?;
            validate_binary_calendar_epoch(raw).map_err(|e| SchemaError::InvalidProperty {
                message: match e {
                    VmError::InvalidValue { message } => message,
                    other => other.to_string(),
                },
            })?;
        }
        if kind == ValueKind::DateTime
            && matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed)
        {
            let Some(bits) = known_binary_length_in_bits(props) else {
                return Ok(());
            };
            let expected = if rep == BinaryNumberRep::BinarySeconds {
                32
            } else {
                64
            };
            if bits != expected {
                let msg = if rep == BinaryNumberRep::BinarySeconds {
                    "Schema Definition Error: binary xs:dateTime must be 32 bits when binaryCalendarRep='binarySeconds'"
                } else {
                    "Schema Definition Error: binary xs:dateTime must be 64 bits when binaryCalendarRep='binaryMilliseconds'"
                };
                return Err(SchemaError::InvalidProperty {
                    message: msg.into(),
                });
            }
        }
        return Ok(());
    }

    if !matches!(
        rep,
        BinaryNumberRep::Bcd | BinaryNumberRep::Ibm4690Packed | BinaryNumberRep::PackedBcd
    ) {
        return Ok(());
    }

    if rep == BinaryNumberRep::PackedBcd && !props.binary_packed_sign_codes_defined {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property binaryPackedSignCodes is not defined.".into(),
        });
    }

    if props.length_kind == LengthKind::Implicit {
        let type_name = binary_prim_type_label(kind, props);
        let msg = if kind == ValueKind::Time {
            alloc::format!(
                "Schema Definition Error: Length of binary data '{type_name}' cannot be determined implicitly"
            )
        } else {
            alloc::format!(
                "Schema Definition Error: Length of binary data '{type_name}' with binaryCalendarRep='{}' cannot be determined implicitly.",
                binary_calendar_rep_name(rep)
            )
        };
        return Err(SchemaError::InvalidProperty { message: msg });
    }

    if props.calendar_pattern_kind != CalendarPatternKind::Explicit {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: calendarPatternKind must be 'explicit' when binaryCalendarRep='{}'",
                binary_calendar_rep_name(rep)
            ),
        });
    }

    if let Some(bits) = known_binary_length_in_bits(props) {
        if bits % 4 != 0 {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: The given length ({bits} bits) must be a multiple of 4 when using binaryCalendarRep='{}'",
                    binary_calendar_rep_name(rep)
                ),
            });
        }
    }

    let pattern = props
        .calendar_pattern
        .and_then(|id| strings.get(id).ok())
        .unwrap_or("");
    if pattern.is_empty() {
        return Ok(());
    }

    let xsd = calendar_xsd_type_label(kind, props.calendar_date_only);
    let allowed = valid_binary_pattern_chars(kind, props.calendar_date_only);
    for ch in pattern.chars() {
        if !allowed.contains(ch) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: Character '{ch}' not allowed in dfdl:calendarPattern for xs:{xsd} with a binaryCalendarRep of '{}'",
                    binary_calendar_rep_name(rep)
                ),
            });
        }
    }
    if pattern.contains("eee") || pattern.contains("MMM") {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: dfdl:calendarPattern must only contain characters that result in the presentation of digits for xs:{xsd} with a binaryCalendarRep of '{}'",
                binary_calendar_rep_name(rep)
            ),
        });
    }
    Ok(())
}

/// Implicit binary length for non-calendar numerics (DFDL 12.3.3).
pub fn validate_implicit_binary_length_schema(
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), SchemaError> {
    if props.representation != Representation::Binary || props.length_kind != LengthKind::Implicit {
        return Ok(());
    }
    if props.input_value_calc.is_some()
        || props.input_value_calc_sibling.is_some()
        || props.input_value_calc_segments.is_some()
    {
        return Ok(());
    }
    if matches!(kind, ValueKind::String | ValueKind::HexBinary | ValueKind::Complex) {
        return Ok(());
    }
    if matches!(kind, ValueKind::DateTime | ValueKind::Time) {
        return validate_binary_calendar_schema(kind, props, strings);
    }
    let implicit_ok = matches!(
        kind,
        ValueKind::Byte
            | ValueKind::UnsignedByte
            | ValueKind::Short
            | ValueKind::UnsignedShort
            | ValueKind::Int
            | ValueKind::UnsignedInt
            | ValueKind::Float
            | ValueKind::Boolean
            | ValueKind::Long
            | ValueKind::Double
    );
    if implicit_ok {
        return Ok(());
    }
    let type_name = binary_prim_type_label(kind, props);
    Err(SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error: Length of binary data '{type_name}' cannot be determined implicitly."
        ),
    })
}

#[cfg(test)]
mod calendar_tests {
    use super::*;

    #[test]
    fn epoch_plus_seconds_date_time_bin() {
        let base = parse_calendar_epoch_unix("1977-01-01T00:00:07").unwrap();
        let out = format_unix_datetime_utc(base + 62);
        assert_eq!(out, "1977-01-01T00:01:09");
    }

    #[test]
    fn binary_calendar_epoch_timezone_display() {
        let base = parse_calendar_epoch_unix("2018-01-01T09:13:42+09:00").unwrap();
        let out = format_binary_calendar_datetime(base + 1, 0, "2018-01-01T09:13:42+09:00");
        assert_eq!(out, "2018-01-01T09:13:43+09:00");
    }

    #[test]
    fn binary_calendar_epoch_utc_zero_offset_suffix() {
        let base = parse_calendar_epoch_unix("1870-01-01T00:05:00+00:00").unwrap();
        let out = format_binary_calendar_datetime(base - 1, 0, "1870-01-01T00:05:00+00:00");
        assert!(out.ends_with("+00:00"), "{out}");
    }

    #[test]
    fn calendar_timezone_property_to_xsd_suffix() {
        assert_eq!(
            calendar_timezone_xsd_suffix("UTC-05:00").as_deref(),
            Some("-05:00")
        );
        assert_eq!(
            calendar_timezone_xsd_suffix("UTC").as_deref(),
            Some("+00:00")
        );
        assert!(calendar_timezone_xsd_suffix("").is_none());
    }

    #[test]
    fn icu_binary_millis_bounds() {
        use crate::length_validate::DaffodilTunables;
        let tunables = DaffodilTunables::default();
        let epoch = "2000-06-15T03:25:19";
        let err = |delta: i64, detail: &str| {
            format_binary_calendar_from_millis_delta(epoch, delta, &tunables)
                .unwrap_err()
                .to_string()
                .contains(detail)
        };
        assert!(err(
            183881207882081001,
            "millis value greater than upper bounds for a Calendar"
        ));
        assert!(err(
            -184304863567519001,
            "millis value less than lower bounds for a Calendar"
        ));
        let tunable = format_binary_calendar_from_millis_delta(
            epoch,
            183881207882081000,
            &tunables,
        )
        .unwrap_err()
        .to_string();
        assert!(tunable.contains("Tunable Limit Exceeded Error"));
        let tunable_min = format_binary_calendar_from_millis_delta(
            epoch,
            -184304863567519000,
            &tunables,
        )
        .unwrap_err()
        .to_string();
        assert!(tunable_min.contains("Tunable Limit Exceeded Error"));
    }
}
