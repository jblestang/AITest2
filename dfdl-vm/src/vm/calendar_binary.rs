use crate::error::VmError;

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
        return (core, -parse_tz_offset_secs(&off[1..]).unwrap_or(0));
    }
    if let Some(idx) = iso[10..].rfind('-') {
        let idx = idx + 10;
        let (core, off) = iso.split_at(idx);
        return (core, parse_tz_offset_secs(&off[1..]).unwrap_or(0));
    }
    (iso, 0)
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
    let secs = (days as i64) * 86400 + (hh as i64) * 3600 + (mm as i64) * 60 + ss as i64;
    Ok(secs)
}

fn days_from_civil(y: i32, m: u32, d: u32) -> Result<u32, VmError> {
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
    Ok((era * 146097 + doe) as u32)
}

pub fn format_unix_datetime_utc(secs: i64) -> alloc::string::String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let hh = (rem / 3600) as u32;
    let mm = ((rem % 3600) / 60) as u32;
    let ss = (rem % 60) as u32;
    let (y, m, d) = civil_from_days(days as i64);
    alloc::format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}")
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

pub fn decode_binary_milliseconds_value(bytes: &[u8], le: bool) -> Result<i64, VmError> {
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
    Ok(v / 1000)
}
