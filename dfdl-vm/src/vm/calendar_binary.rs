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
    if text.len() < 4 || !text.as_bytes()[0..4].iter().all(|b| b.is_ascii_digit()) {
        return Ok(());
    }
    let year: i32 = text[0..4].parse().unwrap_or(0);
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

    if props.length_kind == LengthKind::Implicit {
        let type_name = binary_prim_type_label(kind, props);
        let msg = if matches!(kind, ValueKind::DateTime | ValueKind::Time) {
            alloc::format!(
                "Schema Definition Error: Length of binary data '{type_name}' with binaryCalendarRep='{}' cannot be determined implicitly",
                binary_calendar_rep_name(rep)
            )
        } else {
            alloc::format!(
                "Schema Definition Error: Length of binary data '{type_name}' cannot be determined implicitly"
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
            "Schema Definition Error: Length of binary data '{type_name}' cannot be determined implicitly"
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
}
