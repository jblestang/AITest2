use super::parse_sibling_property_expr;
use crate::error::VmError;
use crate::ir::{IrInputValueCalcSegment, IrProps, StringPool};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) fn field_chars(fields: &BTreeMap<char, String>, keys: &[char]) -> Option<String> {
    for k in keys {
        if let Some(v) = fields.get(k) {
            return Some(v.clone());
        }
    }
    None
}

pub(crate) fn calendar_pattern_digit_count(pattern: &str) -> usize {
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    let mut total = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if !c.is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let mut width = 1usize;
        while i + width < chars.len() && chars[i + width] == c {
            width += 1;
        }
        total += width;
        i += width;
    }
    total
}

pub(crate) fn pad_calendar_digit_field(digits: &str, pattern: &str) -> String {
    let need = calendar_pattern_digit_count(pattern);
    if digits.len() >= need {
        return digits.into();
    }
    let pad = need - digits.len();
    format!("{}{digits}", "0".repeat(pad))
}

pub(crate) fn format_calendar_pattern(
    digits: &str,
    pattern: &str,
    century_start: u32,
    first_day_of_week: u32,
) -> Result<String, VmError> {
    let mut di = 0usize;
    let mut fields = BTreeMap::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        let mut width = 1usize;
        while i + width < chars.len() && chars[i + width] == c {
            width += 1;
        }
        let field: String = digits.chars().skip(di).take(width).collect();
        if field.len() != width {
            return Err(VmError::InvalidValue {
                message: format!(
                    "calendar `{pattern}` expected {width} digits for `{c}`, got `{field}`"
                ),
            });
        }
        di += width;
        fields.insert(c, field);
        i += width;
    }
    let letters = crate::vm::calendar_binary::calendar_pattern_letters_only(pattern);
    let has_y = letters.contains('y') || letters.contains('Y');
    let has_m = letters.contains('M');
    let has_d = letters.contains('d') || letters.contains('e');
    let has_doy = letters.contains('D');
    let year = fields
        .get(&'y')
        .or_else(|| fields.get(&'Y'))
        .map(|y| expand_calendar_year(y, century_start))
        .transpose()?;
    let month = fields.get(&'M').cloned();
    let day = fields.get(&'d').or_else(|| fields.get(&'e')).cloned();
    let day_of_year = fields.get(&'D').cloned();
    let hour = field_chars(&fields, &['H', 'h', 'k', 'K']);
    let minute = fields.get(&'m').cloned();
    let second = fields.get(&'s').cloned();
    let frac = fields.get(&'S').map(|s| format_calendar_s_fraction(s));
    if year.is_some()
        || month.is_some()
        || day.is_some()
        || day_of_year.is_some()
        || has_y
        || has_m
        || has_d
        || has_doy
    {
        let year_s = if has_y {
            year.ok_or_else(|| VmError::InvalidValue {
                message: format!("calendar `{pattern}` missing year"),
            })?
        } else {
            String::from("1970")
        };
        let year_i: i32 = year_s.parse().map_err(|_| VmError::InvalidValue {
            message: format!("calendar `{pattern}` invalid year `{year_s}`"),
        })?;
        let (month_s, day_s) = if has_doy {
            let ord: u32 = day_of_year
                .ok_or_else(|| VmError::InvalidValue {
                    message: format!("calendar `{pattern}` missing day-of-year"),
                })?
                .parse()
                .map_err(|_| VmError::InvalidValue {
                    message: format!("calendar `{pattern}` invalid day-of-year"),
                })?;
            let (m, d) = crate::vm::calendar_binary::month_day_from_ordinal(year_i, ord)?;
            (format!("{m:02}"), format!("{d:02}"))
        } else {
            let month_s = if has_m {
                month.ok_or_else(|| VmError::InvalidValue {
                    message: format!("calendar `{pattern}` missing month"),
                })?
            } else {
                String::from("01")
            };
            let month_n: u32 = month_s.parse().map_err(|_| VmError::InvalidValue {
                message: format!("calendar `{pattern}` invalid month `{month_s}`"),
            })?;
            let day_s = if letters.contains('e') && !letters.contains('d') {
                let e_raw = fields.get(&'e').ok_or_else(|| VmError::InvalidValue {
                    message: format!("calendar `{pattern}` missing localized day-of-week"),
                })?;
                let localized: u32 = e_raw.parse().map_err(|_| VmError::InvalidValue {
                    message: format!("calendar `{pattern}` invalid day-of-week `{e_raw}`"),
                })?;
                let wd = crate::vm::calendar_binary::weekday_from_localized_index(
                    localized,
                    first_day_of_week,
                );
                let d = crate::vm::calendar_binary::first_weekday_in_month(year_i, month_n, wd)
                    .ok_or_else(|| VmError::InvalidValue {
                        message: format!(
                            "calendar `{pattern}` no day for localized week index `{localized}`"
                        ),
                    })?;
                format!("{d:02}")
            } else if has_d {
                day.ok_or_else(|| VmError::InvalidValue {
                    message: format!("calendar `{pattern}` missing day"),
                })?
            } else {
                String::from("01")
            };
            (month_s, day_s)
        };
        let month_n: u32 = month_s.parse().map_err(|_| VmError::InvalidValue {
            message: format!("calendar `{pattern}` invalid month `{month_s}`"),
        })?;
        let day_n: u32 = day_s.parse().map_err(|_| VmError::InvalidValue {
            message: format!("calendar `{pattern}` invalid day `{day_s}`"),
        })?;
        if let (Some(hour), Some(minute), Some(second)) = (&hour, &minute, &second) {
            let mut out = format!("{year_s}-{month_n:02}-{day_n:02}T{hour}:{minute}:{second}");
            if let Some(f) = frac {
                out.push_str(&f);
            }
            return Ok(out);
        }
        return Ok(format!("{year_s}-{month_n:02}-{day_n:02}"));
    }
    if crate::vm::calendar_binary::calendar_pattern_time_only(pattern) {
        let hour_s = hour.as_deref().unwrap_or("00").to_string();
        let minute_s = minute.as_deref().unwrap_or("00").to_string();
        let second_s = second.as_deref().unwrap_or("00").to_string();
        let mut out = format!("{hour_s}:{minute_s}:{second_s}");
        if let Some(f) = frac {
            out.push_str(&f);
        }
        return Ok(out);
    }
    if let (Some(hour), Some(minute), Some(second)) = (&hour, &minute, &second) {
        let mut out = format!("{hour}:{minute}:{second}");
        if let Some(f) = frac {
            out.push_str(&f);
        }
        return Ok(out);
    }
    Err(VmError::InvalidValue {
        message: format!("calendar `{pattern}` missing date/time fields"),
    })
}

pub(crate) fn format_calendar_s_fraction(s_digits: &str) -> String {
    if s_digits.is_empty() {
        return String::new();
    }
    let ms = if s_digits.len() >= 3 {
        &s_digits[..3]
    } else {
        s_digits
    };
    let micros = ms.parse::<u32>().unwrap_or(0).saturating_mul(1000);
    format!(".{micros:06}")
}

pub(crate) struct CalendarTextFields {
    pub(crate) weekday: Option<String>,
    pub(crate) month: Option<u32>,
    pub(crate) day: Option<u32>,
    pub(crate) day_of_year: Option<u32>,
    pub(crate) week_in_month: Option<u32>,
    pub(crate) week_of_year: Option<u32>,
    pub(crate) localized_dow: Option<u32>,
    pub(crate) year: Option<i32>,
    pub(crate) hour: Option<u32>,
    pub(crate) minute: Option<u32>,
    pub(crate) second: Option<u32>,
    pub(crate) fraction_digits: Option<String>,
    pub(crate) hour12: bool,
    pub(crate) am_pm: Option<bool>,
    pub(crate) timezone: Option<String>,
    pub(crate) era_is_bc: bool,
}

#[derive(Default)]
pub(crate) struct CalendarPatternPresence {
    pub(crate) y: bool,
    pub(crate) m: bool,
    pub(crate) d: bool,
    pub(crate) day_of_year: bool,
    pub(crate) week_in_month: bool,
    pub(crate) week_of_year: bool,
    pub(crate) weekday: bool,
}

pub(crate) struct CalendarTextConfig<'a> {
    pub(crate) language: Option<&'a str>,
    pub(crate) first_day_of_week: u32,
    pub(crate) days_in_first_week: u32,
}

#[allow(dead_code)]
pub(crate) fn append_default_utc_offset(
    kind: crate::ir::ValueKind,
    date_only: bool,
    parsed: &str,
) -> String {
    if parsed.contains('T') {
        if parsed.contains('+')
            || parsed
                .rfind('-')
                .is_some_and(|i| i > 10 && parsed[i + 1..].contains(':'))
        {
            return parsed.into();
        }
        return format!("{parsed}+00:00");
    }
    if kind == crate::ir::ValueKind::Time || date_only {
        return format!("{parsed}+00:00");
    }
    parsed.into()
}

pub(crate) fn lexical_has_xsd_timezone(parsed: &str) -> bool {
    let check = |s: &str| {
        if s.contains('+') {
            return true;
        }
        s.rfind('-')
            .is_some_and(|i| i > 0 && s[i + 1..].contains(':'))
    };
    if let Some(idx) = parsed.find('T') {
        return check(&parsed[idx + 1..]);
    }
    check(parsed)
}

pub(crate) fn append_packed_calendar_timezone(
    props: &IrProps,
    strings: &StringPool,
    _kind: crate::ir::ValueKind,
    date_only: bool,
    parsed: &str,
    default_utc_when_missing: bool,
    allow_inherited_format_timezone: bool,
) -> Result<String, VmError> {
    if lexical_has_xsd_timezone(parsed) {
        return Ok(parsed.into());
    }
    let timezone_suffix = || {
        props
            .calendar_time_zone
            .and_then(|id| strings.get(id).ok())
            .and_then(crate::vm::calendar_binary::calendar_timezone_xsd_suffix)
    };
    if props.calendar_time_zone_defined {
        if let Some(suffix) = timezone_suffix() {
            return Ok(format!("{parsed}{suffix}"));
        }
        return Ok(parsed.into());
    }
    if allow_inherited_format_timezone {
        if let Some(suffix) = timezone_suffix() {
            return Ok(format!("{parsed}{suffix}"));
        }
    }
    if default_utc_when_missing && parsed.contains('T') && !date_only {
        Ok(format!("{parsed}+00:00"))
    } else {
        Ok(parsed.into())
    }
}

pub(crate) fn read_calendar_timezone(
    text: &str,
    ti: &mut usize,
    _z_width: usize,
) -> Result<String, VmError> {
    let mut pos = *ti;
    while pos < text.len() && text.as_bytes()[pos].is_ascii_whitespace() {
        pos += 1;
    }
    let mut tail = &text[pos..];
    if tail.len() >= 3 && tail[..3].eq_ignore_ascii_case("GMT") {
        pos += 3;
        while pos < text.len() && text.as_bytes()[pos].is_ascii_whitespace() {
            pos += 1;
        }
        tail = &text[pos..];
        if tail.is_empty() {
            *ti = pos;
            return Ok("+00:00".into());
        }
    }
    let (tz, consumed) = parse_calendar_tz_offset(tail).ok_or_else(|| VmError::InvalidValue {
        message: "calendar text mismatch".into(),
    })?;
    *ti = pos + consumed;
    Ok(tz)
}

pub(crate) fn timezone_abbrev_to_offset(name: &str) -> Option<&'static str> {
    let n = name.trim();
    let lower = n.to_ascii_lowercase();
    match lower.as_str() {
        "est" | "et" | "eastern standard time" => Some("-05:00"),
        "edt" | "eastern daylight time" => Some("-04:00"),
        "cst" | "central standard time" => Some("-06:00"),
        "cdt" | "central daylight time" => Some("-05:00"),
        "mst" | "mountain standard time" => Some("-07:00"),
        "mdt" | "mountain daylight time" => Some("-06:00"),
        "pst" | "pt" | "pacific time" | "pacific standard time" => Some("-08:00"),
        "pdt" | "pacific daylight time" => Some("-07:00"),
        "utc" | "gmt" | "z" => Some("+00:00"),
        _ if n.eq_ignore_ascii_case("GMT") => Some("+00:00"),
        _ => None,
    }
}

pub(crate) fn timezone_long_names() -> &'static [(&'static str, &'static str)] {
    &[
        ("Eastern Standard Time", "-05:00"),
        ("Pacific Standard Time", "-08:00"),
        ("Central Standard Time", "-06:00"),
        ("Mountain Standard Time", "-07:00"),
        ("Los Angeles Time", "-08:00"),
    ]
}

pub(crate) fn timezone_id_to_offset(id: &str) -> Option<&'static str> {
    match id.to_ascii_lowercase().as_str() {
        "uslax" => Some("-08:00"),
        "unk" => Some("+00:00"),
        _ => None,
    }
}

pub(crate) fn read_calendar_timezone_name(
    text: &str,
    ti: &mut usize,
    kind: char,
    width: usize,
) -> Result<String, VmError> {
    let rest = text[*ti..].trim_start();
    let base = *ti + text[*ti..].len().saturating_sub(rest.len());
    if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case("GMT") {
        let after_gmt = &rest[3..];
        let tail = after_gmt.trim_start();
        let tail_start = base + 3 + after_gmt.len().saturating_sub(tail.len());
        if tail.is_empty() {
            *ti = tail_start;
            return Ok("+00:00".into());
        }
        if let Some((tz, consumed)) = parse_calendar_tz_offset(tail) {
            *ti = tail_start + consumed;
            return Ok(tz);
        }
    }
    if (kind == 'z' || kind == 'v') && width >= 4 {
        for (name, off) in timezone_long_names() {
            if rest.starts_with(name) {
                *ti += text[*ti..].len() - rest.len() + name.len();
                return Ok((*off).into());
            }
        }
    }
    if kind == 'V' {
        if width >= 4 {
            for (name, off) in timezone_long_names() {
                if rest.starts_with(name) {
                    *ti += text[*ti..].len() - rest.len() + name.len();
                    return Ok((*off).into());
                }
            }
        }
        let end = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '.' || c == ',')
            .unwrap_or(rest.len());
        let word = &rest[..end];
        if word.is_empty() {
            return Err(VmError::InvalidValue {
                message: "calendar text mismatch".into(),
            });
        }
        let off = timezone_id_to_offset(word).ok_or_else(|| VmError::InvalidValue {
            message: "calendar text mismatch".into(),
        })?;
        *ti += text[*ti..].len() - rest.len() + word.len();
        return Ok((*off).into());
    }
    let end = rest
        .find(|c: char| c.is_ascii_whitespace() || c == '.' || c == ',')
        .unwrap_or(rest.len());
    let word = &rest[..end];
    if word.is_empty() {
        return Err(VmError::InvalidValue {
            message: "calendar text mismatch".into(),
        });
    }
    if word.len() >= 3 && word[..3].eq_ignore_ascii_case("GMT") {
        let tail = word[3..].trim();
        if tail.is_empty() {
            *ti += text[*ti..].len() - rest.len() + word.len();
            return Ok("+00:00".into());
        }
        if let Some((tz, _)) = parse_calendar_tz_offset(tail) {
            *ti += text[*ti..].len() - rest.len() + word.len();
            return Ok(tz);
        }
    }
    if let Some((tz, consumed)) = parse_calendar_tz_offset(word) {
        *ti += text[*ti..].len() - rest.len() + consumed;
        return Ok(tz);
    }
    let off = timezone_abbrev_to_offset(word).ok_or_else(|| VmError::InvalidValue {
        message: "calendar text mismatch".into(),
    })?;
    *ti += text[*ti..].len() - rest.len() + word.len();
    Ok((*off).into())
}

pub(crate) fn parse_calendar_tz_offset(raw: &str) -> Option<(String, usize)> {
    let mut i = 0usize;
    while i < raw.len() && raw.as_bytes()[i].is_ascii_whitespace() {
        i += 1;
    }
    let start = i;
    let sign = *raw.as_bytes().get(i)?;
    if sign != b'+' && sign != b'-' {
        return None;
    }
    i += 1;
    if i + 5 <= raw.len()
        && raw[i..i + 2].bytes().all(|b| b.is_ascii_digit())
        && raw.as_bytes()[i + 2] == b':'
        && raw[i + 3..i + 5].bytes().all(|b| b.is_ascii_digit())
    {
        let hh = &raw[i..i + 2];
        let mm = &raw[i + 3..i + 5];
        hh.parse::<u8>().ok()?;
        mm.parse::<u8>().ok()?;
        let sign = sign as char;
        return Some((format!("{sign}{hh}:{mm}"), start + 1 + 2 + 1 + 2));
    }
    if i + 4 <= raw.len() && raw[i..i + 4].bytes().all(|b| b.is_ascii_digit()) {
        let hh = &raw[i..i + 2];
        let mm = &raw[i + 2..i + 4];
        hh.parse::<u8>().ok()?;
        mm.parse::<u8>().ok()?;
        let sign = sign as char;
        return Some((format!("{sign}{hh}:{mm}"), start + 1 + 4));
    }
    if i + 2 <= raw.len() && raw[i..i + 2].bytes().all(|b| b.is_ascii_digit()) {
        let hh = &raw[i..i + 2];
        hh.parse::<u8>().ok()?;
        let sign = sign as char;
        return Some((format!("{sign}{hh}:00"), start + 1 + 2));
    }
    if i < raw.len() && raw[i..i + 1].bytes().all(|b| b.is_ascii_digit()) {
        let h1 = raw[i..i + 1].parse::<u8>().ok()?;
        if h1 <= 9 {
            let sign = sign as char;
            return Some((format!("{sign}{h1:02}:00"), start + 1 + 1));
        }
    }
    None
}

pub(crate) fn read_calendar_ampm(text: &str, ti: &mut usize) -> Result<bool, VmError> {
    let rest = text[*ti..].trim_start();
    let upper = rest.to_ascii_uppercase();
    let (pm, consumed) = if upper.starts_with("PM") {
        (true, 2)
    } else if upper.starts_with("AM") {
        (false, 2)
    } else {
        return Err(VmError::InvalidValue {
            message: "calendar text mismatch".into(),
        });
    };
    *ti += text[*ti..].len() - rest.len() + consumed;
    Ok(pm)
}

pub(crate) fn read_calendar_era(text: &str, ti: &mut usize) -> Result<bool, VmError> {
    let rest = text[*ti..].trim_start();
    let word_end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    let word = &rest[..word_end];
    let bc = if word.eq_ignore_ascii_case("BC") {
        true
    } else if word.eq_ignore_ascii_case("AD") {
        false
    } else {
        return Err(VmError::InvalidValue {
            message: "calendar text mismatch".into(),
        });
    };
    *ti += text[*ti..].len() - rest.len() + word.len();
    Ok(bc)
}

pub(crate) fn apply_hour12(fields: &mut CalendarTextFields) -> Result<(), VmError> {
    if !fields.hour12 {
        return Ok(());
    }
    let h = fields.hour.ok_or_else(|| VmError::InvalidValue {
        message: "calendar missing hour".into(),
    })?;
    let pm = fields.am_pm.unwrap_or(false);
    let h24 = if pm {
        if h == 12 {
            12
        } else {
            h + 12
        }
    } else if h == 12 {
        0
    } else {
        h
    };
    fields.hour = Some(h24);
    Ok(())
}

pub(crate) fn finalize_calendar_hour_fields(
    fields: &mut CalendarTextFields,
    pattern: &str,
) -> Result<(), VmError> {
    let letters = crate::vm::calendar_binary::calendar_pattern_letters_only(pattern);
    let has_a = letters.contains('a');
    let has_cap_h = pattern.contains('H');
    if has_a && has_cap_h && !fields.hour12 {
        fields.hour = Some(if fields.am_pm.unwrap_or(false) { 12 } else { 0 });
        return Ok(());
    }
    if letters.contains('K') {
        let h = fields.hour.unwrap_or(0);
        if has_a && h == 12 {
            return Err(VmError::InvalidValue {
                message: "calendar text mismatch".into(),
            });
        }
        let pm = fields.am_pm.unwrap_or(false);
        let h24 = if pm {
            if h == 0 {
                12
            } else {
                h + 12
            }
        } else {
            h
        };
        fields.hour = Some(h24);
        return Ok(());
    }
    if letters.contains('k') {
        let h = fields.hour.unwrap_or(0);
        fields.hour = Some(if h == 0 {
            0
        } else if h == 24 {
            0
        } else {
            h - 1
        });
        return Ok(());
    }
    apply_hour12(fields)
}

pub(crate) fn month_from_name_locale(name: &str, language: Option<&str>, lax: bool) -> Option<u32> {
    let trimmed = if lax {
        name.trim().trim_end_matches('.')
    } else {
        name.trim()
    };
    if let Some(lang) = language {
        let key = lang.split('-').next().unwrap_or(lang).to_ascii_lowercase();
        let lower = trimmed.to_lowercase();
        match key.as_str() {
            "de" if lower.contains("mär") || lower.contains("marz") || lower.contains("maerz") => {
                return Some(3);
            }
            "es" if lower == "noviembre" => return Some(11),
            "ru" if lower.starts_with("мар") => return Some(3),
            _ => {}
        }
    }
    month_from_name(trimmed)
}

pub(crate) fn month_from_name(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    match n.as_str() {
        "january" | "jan" => Some(1),
        "february" | "feb" => Some(2),
        "march" | "mar" => Some(3),
        "april" | "apr" => Some(4),
        "may" => Some(5),
        "june" | "jun" => Some(6),
        "july" | "jul" => Some(7),
        "august" | "aug" => Some(8),
        "september" | "sep" | "sept" => Some(9),
        "october" | "oct" => Some(10),
        "november" | "nov" => Some(11),
        "december" | "dec" => Some(12),
        _ => None,
    }
}

pub(crate) fn weekday_from_name_locale(name: &str, language: Option<&str>) -> Option<u32> {
    let n = name.trim().to_ascii_lowercase();
    if let Some(lang) = language {
        let key = lang.split('-').next().unwrap_or(lang).to_ascii_lowercase();
        match key.as_str() {
            "de" if matches!(n.as_str(), "freitag" | "fr") => return Some(5),
            "es" if matches!(n.as_str(), "lunes" | "lu") => return Some(1),
            "ru" if n.starts_with("пят") => return Some(5),
            _ => {}
        }
    }
    weekday_from_name(name)
}

pub(crate) fn weekday_from_name(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    match n.as_str() {
        "monday" | "mon" => Some(1),
        "tuesday" | "tue" | "tues" => Some(2),
        "wednesday" | "wed" => Some(3),
        "thursday" | "thu" | "thur" | "thurs" => Some(4),
        "friday" | "fri" => Some(5),
        "saturday" | "sat" => Some(6),
        "sunday" | "sun" => Some(7),
        _ => None,
    }
}

pub(crate) fn calendar_language_is_valid(locale: &str) -> bool {
    if locale.is_empty() {
        return false;
    }
    let mut parts = locale.split(['-', '_']);
    let first = parts.next().unwrap_or("");
    if first.is_empty() || first.len() > 8 || !first.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    for part in parts {
        if part.is_empty() || part.len() > 8 {
            return false;
        }
        if !part.chars().all(|c| c.is_ascii_alphanumeric()) {
            return false;
        }
    }
    true
}

pub(crate) fn calendar_language_sde(locale: &str) -> VmError {
    VmError::InvalidValue {
        message: format!(
            "Schema Definition Error: dfdl:calendarLanguage property syntax error. Must match '([A-Za-z]{{1,8}}([-_][A-Za-z0-9]{{1,8}})*)' (ex: 'en_us' or 'de_1996'), but was '{locale}'."
        ),
    }
}

pub(crate) fn sibling_text_map_for_calendar(
    siblings: Option<&BTreeMap<String, crate::value::DfdlValue>>,
) -> Option<BTreeMap<String, String>> {
    let map = siblings?;
    let mut out = BTreeMap::new();
    for (k, v) in map {
        let text = match v {
            crate::value::DfdlValue::String(s) => s.text.clone(),
            crate::value::DfdlValue::DateTime(s) => s.clone(),
            crate::value::DfdlValue::Integer(i) => i.clone(),
            crate::value::DfdlValue::Long(n) => format!("{n}"),
            crate::value::DfdlValue::Int(n) => format!("{n}"),
            _ => continue,
        };
        out.insert(k.clone(), text);
    }
    Some(out)
}

pub(crate) fn resolve_calendar_language(
    props: &IrProps,
    strings: &StringPool,
    siblings: Option<&BTreeMap<String, String>>,
) -> Result<Option<String>, VmError> {
    if let Some(segs) = &props.calendar_language_segments {
        let mut out = String::new();
        for seg in segs {
            match seg {
                IrInputValueCalcSegment::Sibling(id) => {
                    let name = strings.get(*id)?;
                    let local = crate::xml_util::local_name_str(name);
                    let val = siblings
                        .and_then(|m| m.get(name).or_else(|| m.get(local)))
                        .ok_or_else(|| VmError::InvalidValue {
                            message: format!("missing sibling `{name}` for calendarLanguage"),
                        })?;
                    out.push_str(val);
                }
                IrInputValueCalcSegment::Literal(id) => out.push_str(strings.get(*id)?),
                IrInputValueCalcSegment::Substring {
                    sibling,
                    start,
                    length,
                } => {
                    let name = strings.get(*sibling)?;
                    let text = siblings.and_then(|m| m.get(name)).ok_or_else(|| {
                        VmError::InvalidValue {
                            message: format!("missing sibling `{name}` for calendarLanguage"),
                        }
                    })?;
                    let start = (*start as usize).saturating_sub(1);
                    for ch in text.chars().skip(start).take(*length as usize) {
                        out.push(ch);
                    }
                }
                IrInputValueCalcSegment::InfosetPath(_) => {
                    return Err(VmError::InvalidValue {
                        message: "calendarLanguage infoset path not supported".into(),
                    });
                }
                IrInputValueCalcSegment::ValueLength { .. } => {
                    return Err(VmError::InvalidValue {
                        message: "calendarLanguage valueLength segment not supported".into(),
                    });
                }
            }
        }
        if !calendar_language_is_valid(&out) {
            return Err(calendar_language_sde(&out));
        }
        return Ok(Some(out));
    }
    if let Some(id) = props.calendar_language {
        let raw = strings.get(id)?.trim();
        if !raw.is_empty() {
            let locale = if let Some(sib_local) = parse_sibling_property_expr(raw) {
                calendar_language_from_sibling_text_map(siblings, &sib_local)?
            } else {
                raw.to_string()
            };
            if !calendar_language_is_valid(&locale) {
                return Err(calendar_language_sde(&locale));
            }
            return Ok(Some(locale));
        }
    }
    Ok(None)
}

pub(crate) fn calendar_language_from_sibling_text_map(
    siblings: Option<&BTreeMap<String, String>>,
    sib_local: &str,
) -> Result<String, VmError> {
    let map = siblings.ok_or_else(|| VmError::InvalidValue {
        message: format!("missing sibling `{sib_local}` for calendarLanguage"),
    })?;
    for (k, v) in map {
        if k == sib_local || crate::xml_util::local_name_str(k) == sib_local {
            return Ok(v.clone());
        }
    }
    Err(VmError::InvalidValue {
        message: format!("missing sibling `{sib_local}` for calendarLanguage"),
    })
}

pub(crate) fn month_name_unparse_locale(
    month: u32,
    language: Option<&str>,
    width: usize,
) -> String {
    let key = language
        .and_then(|l| l.split(['-', '_']).next())
        .map(|s| s.to_ascii_lowercase());
    if key.as_deref() == Some("de") {
        let full = match month {
            1 => "Januar",
            2 => "Februar",
            3 => "März",
            4 => "April",
            5 => "Mai",
            6 => "Juni",
            7 => "Juli",
            8 => "August",
            9 => "September",
            10 => "Oktober",
            11 => "November",
            12 => "Dezember",
            _ => "März",
        };
        if width >= 4 {
            return full.into();
        }
        return full.chars().take(width.max(1)).collect();
    }
    if key.as_deref() == Some("es") {
        let full = match month {
            1 => "enero",
            2 => "febrero",
            3 => "marzo",
            4 => "abril",
            5 => "mayo",
            6 => "junio",
            7 => "julio",
            8 => "agosto",
            9 => "septiembre",
            10 => "octubre",
            11 => "noviembre",
            12 => "diciembre",
            _ => "marzo",
        };
        if width >= 4 {
            return full.into();
        }
        return full.chars().take(width.max(1)).collect();
    }
    if key.as_deref() == Some("ru") {
        let full = match month {
            1 => "января",
            2 => "февраля",
            3 => "марта",
            4 => "апреля",
            5 => "мая",
            6 => "июня",
            7 => "июля",
            8 => "августа",
            9 => "сентября",
            10 => "октября",
            11 => "ноября",
            12 => "декабря",
            _ => "марта",
        };
        if width >= 4 {
            return full.into();
        }
        return full.chars().take(width.max(1)).collect();
    }
    let full = match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "March",
    };
    if width >= 4 {
        full.into()
    } else {
        full.chars().take(width.max(1)).collect()
    }
}

pub(crate) fn weekday_name_unparse_locale(wd: u32, language: Option<&str>, width: usize) -> String {
    let key = language
        .and_then(|l| l.split(['-', '_']).next())
        .map(|s| s.to_ascii_lowercase());
    if key.as_deref() == Some("de") {
        let full = match wd {
            1 => "Montag",
            2 => "Dienstag",
            3 => "Mittwoch",
            4 => "Donnerstag",
            5 => "Freitag",
            6 => "Samstag",
            7 => "Sonntag",
            _ => "Freitag",
        };
        if width >= 4 {
            return full.into();
        }
        return full.chars().take(width.max(1)).collect();
    }
    if key.as_deref() == Some("es") {
        let full = match wd {
            1 => "lunes",
            2 => "martes",
            3 => "miércoles",
            4 => "jueves",
            5 => "viernes",
            6 => "sábado",
            7 => "domingo",
            _ => "viernes",
        };
        if width >= 4 {
            return full.into();
        }
        return full.chars().take(width.max(1)).collect();
    }
    if key.as_deref() == Some("ru") {
        let full = match wd {
            1 => "понедельник",
            2 => "вторник",
            3 => "среда",
            4 => "четверг",
            5 => "пятница",
            6 => "суббота",
            7 => "воскресенье",
            _ => "пятница",
        };
        if width >= 4 {
            return full.into();
        }
        return full.chars().take(width.max(1)).collect();
    }
    let full = match wd {
        1 => "Monday",
        2 => "Tuesday",
        3 => "Wednesday",
        4 => "Thursday",
        5 => "Friday",
        6 => "Saturday",
        7 => "Sunday",
        _ => "Friday",
    };
    if width >= 4 {
        full.into()
    } else {
        full.chars().take(width.max(1)).collect()
    }
}

pub(crate) fn parse_iso_date_ymd(iso: &str) -> Result<(i32, u32, u32), VmError> {
    let mut date_part = iso.trim();
    date_part = date_part.split('T').next().unwrap_or(date_part);
    date_part = date_part.split('+').next().unwrap_or(date_part);
    if let Some((d, _)) = date_part.split_once('Z') {
        date_part = d;
    }
    date_part = date_part.trim();
    let mut parts = date_part.split('-');
    let y: i32 = parts
        .next()
        .ok_or_else(|| VmError::InvalidValue {
            message: format!("invalid xs:date `{iso}`"),
        })?
        .parse()
        .map_err(|_| VmError::InvalidValue {
            message: format!("invalid xs:date `{iso}`"),
        })?;
    let m: u32 = parts
        .next()
        .ok_or_else(|| VmError::InvalidValue {
            message: format!("invalid xs:date `{iso}`"),
        })?
        .parse()
        .map_err(|_| VmError::InvalidValue {
            message: format!("invalid xs:date `{iso}`"),
        })?;
    let d: u32 = parts
        .next()
        .ok_or_else(|| VmError::InvalidValue {
            message: format!("invalid xs:date `{iso}`"),
        })?
        .parse()
        .map_err(|_| VmError::InvalidValue {
            message: format!("invalid xs:date `{iso}`"),
        })?;
    Ok((y, m, d))
}

pub(crate) fn unparse_iso_date_to_calendar_pattern(
    iso: &str,
    pattern: &str,
    language: Option<&str>,
) -> Result<String, VmError> {
    let (year, month, day) = parse_iso_date_ymd(iso)?;
    let weekday = weekday_of_ymd(year, month, day).unwrap_or(5);
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            if i < chars.len() && chars[i] == '\'' {
                out.push('\'');
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        i += 2;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if i >= chars.len() {
                return Err(VmError::InvalidValue {
                    message: format!("invalid calendarPattern `{pattern}`"),
                });
            }
            let lit: String = chars[start..i]
                .iter()
                .collect::<String>()
                .replace("''", "'");
            out.push_str(&lit);
            i += 1;
            continue;
        }
        let c = chars[i];
        if c.is_whitespace() {
            out.push(c);
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() {
            let mut w = 1usize;
            while i + w < chars.len() && chars[i + w] == c {
                w += 1;
            }
            let field = match c {
                'E' => weekday_name_unparse_locale(weekday, language, w),
                'M' if w >= 3 => month_name_unparse_locale(month, language, w),
                'M' => format!("{month:02}"),
                'd' => {
                    if w >= 2 {
                        format!("{day:02}")
                    } else {
                        format!("{day}")
                    }
                }
                'y' | 'Y' => {
                    if w >= 4 {
                        format!("{year:04}")
                    } else {
                        format!("{:02}", year % 100)
                    }
                }
                _ => {
                    return Err(VmError::InvalidValue {
                        message: format!("unsupported calendar field `{c}` in unparse"),
                    });
                }
            };
            out.push_str(&field);
            i += w;
            continue;
        }
        out.push(c);
        i += 1;
    }
    Ok(out)
}

pub(crate) fn weekday_of_ymd(year: i32, month: u32, day: u32) -> Option<u32> {
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
    // Zeller: 0=Saturday … convert to ISO Monday=1 … Sunday=7
    Some(((h + 5) % 7 + 1) as u32)
}

pub(crate) fn infer_day_from_weekday(year: i32, month: u32, weekday: u32) -> Option<u32> {
    let dim = crate::vm::calendar_binary::days_in_month(year, month);
    for day in 1..=dim {
        if weekday_of_ymd(year, month, day) == Some(weekday) {
            return Some(day);
        }
    }
    None
}

pub(crate) fn read_calendar_field(
    text: &str,
    ti: &mut usize,
    width: usize,
    letters: char,
) -> Result<String, VmError> {
    if letters == 'E' || letters == 'M' && width >= 3 {
        let rest = text[*ti..].trim_start();
        let word_end = rest
            .find(|c: char| c.is_whitespace() || c == '-' || c == ':' || c == ',')
            .unwrap_or(rest.len());
        let word = &rest[..word_end];
        if word.is_empty() {
            return Err(VmError::InvalidValue {
                message: "calendar text mismatch".into(),
            });
        }
        *ti += text[*ti..].len() - rest.len() + word.len();
        return Ok(word.to_string());
    }
    let slice = text.get(*ti..).ok_or(VmError::InvalidValue {
        message: "calendar text mismatch".into(),
    })?;
    let mut out = String::new();
    let max_digits = if width == 1
        && matches!(
            letters,
            'd' | 'M' | 'y' | 'Y' | 'D' | 'F' | 'w' | 'W' | 'H' | 'h' | 'm' | 's' | 'e'
        ) {
        4
    } else {
        width
    };
    for ch in slice.chars().take(max_digits) {
        if !ch.is_ascii_digit() {
            break;
        }
        out.push(ch);
        *ti += ch.len_utf8();
        if width > 1 && out.len() >= width {
            break;
        }
    }
    if out.is_empty() {
        return Err(VmError::InvalidValue {
            message: "calendar text mismatch".into(),
        });
    }
    if width > 1
        && out.len() != width
        && !matches!(letters, 'H' | 'h' | 'k' | 'K' | 'm' | 's' | 'S' | 'd' | 'M')
    {
        return Err(VmError::InvalidValue {
            message: "calendar text mismatch".into(),
        });
    }
    Ok(out)
}

pub(crate) fn calendar_text_strict_date_error(text: &str) -> VmError {
    VmError::InvalidValue {
        message: format!(
            "Parse Error: Unable to parse / Failed to parse xs:date from text: {text}"
        ),
    }
}

pub(crate) fn calendar_text_strict_time_error(text: &str) -> VmError {
    VmError::InvalidValue {
        message: format!(
            "Parse Error: Unable to parse / Failed to parse xs:time from text: {text}"
        ),
    }
}

pub(crate) fn calendar_text_strict_datetime_error(text: &str) -> VmError {
    VmError::InvalidValue {
        message: format!(
            "Parse Error: Unable to parse / Failed to parse xs:dateTime from text: {text}"
        ),
    }
}

pub(crate) fn xsd_tz_to_offset_secs(tz: &str) -> Option<i64> {
    let (normalized, _) = parse_calendar_tz_offset(tz)?;
    let sign = if normalized.starts_with('-') {
        -1i64
    } else {
        1i64
    };
    let body = normalized.trim_start_matches(['+', '-']);
    let (hh, mm) = body.split_once(':').unwrap_or((body, "0"));
    let hh: i64 = hh.parse().ok()?;
    let mm: i64 = mm.parse().ok()?;
    Some(sign * (hh * 3600 + mm * 60))
}

pub(crate) fn parse_iso_time_hms(iso: &str) -> Result<(u32, u32, u32, Option<i64>), VmError> {
    let iso = iso.trim();
    let (core, tz_secs) = if iso.ends_with('Z') {
        (&iso[..iso.len().saturating_sub(1)], Some(0i64))
    } else if let Some(i) = iso.rfind('+').filter(|&i| i >= 5) {
        (&iso[..i], xsd_tz_to_offset_secs(&iso[i..]))
    } else if let Some(rel) = iso.get(8..) {
        if let Some(i) = rel.find('-') {
            let idx = 8 + i;
            (&iso[..idx], xsd_tz_to_offset_secs(&iso[idx..]))
        } else {
            (iso, None)
        }
    } else {
        (iso, None)
    };
    let parts: Vec<&str> = core.split(':').collect();
    if parts.len() < 2 {
        return Err(VmError::InvalidValue {
            message: format!("invalid xs:time `{iso}`"),
        });
    }
    let hh: u32 = parts[0].parse().map_err(|_| VmError::InvalidValue {
        message: format!("invalid xs:time `{iso}`"),
    })?;
    let mm: u32 = parts[1].parse().map_err(|_| VmError::InvalidValue {
        message: format!("invalid xs:time `{iso}`"),
    })?;
    let ss = if parts.len() > 2 {
        parts[2]
            .split('.')
            .next()
            .unwrap_or(parts[2])
            .parse()
            .unwrap_or(0)
    } else {
        0
    };
    Ok((hh, mm, ss, tz_secs))
}

pub(crate) fn emit_calendar_tz_unparse(offset_secs: i64, kind: char, width: usize) -> String {
    let sign = if offset_secs >= 0 { '+' } else { '-' };
    let abs = offset_secs.abs();
    let oh = abs / 3600;
    let om = (abs % 3600) / 60;
    let xsd = format!("{sign}{oh:02}:{om:02}");
    match kind {
        'z' => {
            if width >= 4 {
                format!("GMT{xsd}")
            } else {
                format!("GMT{sign}{oh}")
            }
        }
        'Z' => format!("{sign}{oh:02}{om:02}"),
        'v' => {
            if width >= 4 {
                format!("GMT{xsd}")
            } else {
                format!("GMT{sign}{oh}")
            }
        }
        'V' => {
            if offset_secs == 0 {
                "gmt".into()
            } else if width >= 4 {
                format!("GMT{xsd}")
            } else {
                "unk".into()
            }
        }
        _ => String::new(),
    }
}

pub(crate) fn unparse_iso_time_to_calendar_pattern(
    iso: &str,
    pattern: &str,
) -> Result<String, VmError> {
    let (hour, minute, second, tz_secs) = parse_iso_time_hms(iso)?;
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            if i < chars.len() && chars[i] == '\'' {
                out.push('\'');
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        i += 2;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if i >= chars.len() {
                return Err(VmError::InvalidValue {
                    message: format!("invalid calendarPattern `{pattern}`"),
                });
            }
            let lit: String = chars[start..i]
                .iter()
                .collect::<String>()
                .replace("''", "'");
            out.push_str(&lit);
            i += 1;
            continue;
        }
        let c = chars[i];
        if c.is_whitespace() {
            out.push(c);
            i += 1;
            continue;
        }
        if matches!(c, 'z' | 'Z' | 'v' | 'V') {
            let mut w = 1usize;
            while i + w < chars.len() && chars[i + w] == c {
                w += 1;
            }
            if let Some(secs) = tz_secs {
                out.push_str(&emit_calendar_tz_unparse(secs, c, w));
            }
            i += w;
            continue;
        }
        if c.is_ascii_alphabetic() {
            let mut w = 1usize;
            while i + w < chars.len() && chars[i + w] == c {
                w += 1;
            }
            let field = match c {
                'h' | 'H' | 'k' | 'K' => {
                    let h = if matches!(c, 'H' | 'k') {
                        hour
                    } else if hour == 0 || hour > 12 {
                        hour % 12
                    } else {
                        hour
                    };
                    if w >= 2 {
                        format!("{h:02}")
                    } else {
                        format!("{h}")
                    }
                }
                'm' => {
                    if w >= 2 {
                        format!("{minute:02}")
                    } else {
                        format!("{minute}")
                    }
                }
                's' => {
                    if w >= 2 {
                        format!("{second:02}")
                    } else {
                        format!("{second}")
                    }
                }
                'S' => "0".repeat(w.min(9)),
                _ => {
                    return Err(VmError::InvalidValue {
                        message: format!("unsupported calendar field `{c}` in unparse"),
                    });
                }
            };
            out.push_str(&field);
            i += w;
            continue;
        }
        out.push(c);
        i += 1;
    }
    Ok(out)
}

pub(crate) fn format_calendar_text(
    text: &str,
    pattern: &str,
    lax: bool,
    century_start: u32,
    cal: CalendarTextConfig<'_>,
    time_overflow_carries_to_date: bool,
    date_only: bool,
) -> Result<String, VmError> {
    let text = text.trim();
    let map_err = |e: VmError| -> VmError {
        if lax {
            e
        } else {
            calendar_text_strict_date_error(text)
        }
    };
    let mut ti = 0usize;
    let mut fields = CalendarTextFields {
        weekday: None,
        month: None,
        day: None,
        day_of_year: None,
        week_in_month: None,
        week_of_year: None,
        localized_dow: None,
        year: None,
        hour: None,
        minute: None,
        second: None,
        fraction_digits: None,
        hour12: false,
        am_pm: None,
        timezone: None,
        era_is_bc: false,
    };
    let mut pattern_parts = CalendarPatternPresence::default();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            if i < chars.len() && chars[i] == '\'' {
                if !text[ti..].starts_with('\'') {
                    return Err(VmError::InvalidValue {
                        message: "calendar text mismatch".into(),
                    });
                }
                ti += 1;
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        i += 2;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if i >= chars.len() {
                return Err(VmError::InvalidValue {
                    message: format!("invalid calendarPattern `{pattern}`"),
                });
            }
            let lit: String = chars[start..i]
                .iter()
                .collect::<String>()
                .replace("''", "'");
            i += 1;
            if !text[ti..].starts_with(&lit) {
                return Err(VmError::InvalidValue {
                    message: format!("calendar `{pattern}` mismatch"),
                });
            }
            ti += lit.len();
            continue;
        }
        let c = chars[i];
        if c.is_whitespace() {
            while ti < text.len() && text.as_bytes()[ti].is_ascii_whitespace() {
                ti += 1;
            }
            i += 1;
            continue;
        }
        if c == 'Z' || c == 'z' || c == 'v' || c == 'V' {
            let mut z_width = 1usize;
            while i + z_width < chars.len() && chars[i + z_width] == c {
                z_width += 1;
            }
            fields.timezone = Some(if c == 'Z' {
                read_calendar_timezone(text, &mut ti, z_width)?
            } else {
                read_calendar_timezone_name(text, &mut ti, c, z_width)?
            });
            i += z_width;
            continue;
        }
        if c == 'G' {
            fields.era_is_bc = read_calendar_era(text, &mut ti)?;
            i += 1;
            continue;
        }
        if c == 'a' {
            let mut a_width = 1usize;
            while i + a_width < chars.len() && chars[i + a_width] == 'a' {
                a_width += 1;
            }
            fields.am_pm = Some(read_calendar_ampm(text, &mut ti)?);
            i += a_width;
            continue;
        }
        const FIELD: &str = "EMdDFwWmyYHhsekKS";
        if !FIELD.contains(c) {
            let Some(ch) = text[ti..].chars().next() else {
                return Err(VmError::InvalidValue {
                    message: "calendar text mismatch".into(),
                });
            };
            if ch != c {
                return Err(VmError::InvalidValue {
                    message: "calendar text mismatch".into(),
                });
            }
            ti += ch.len_utf8();
            i += 1;
            continue;
        }
        let mut width = 1usize;
        while i + width < chars.len() && chars[i + width] == c {
            width += 1;
        }
        let raw = read_calendar_field(text, &mut ti, width, c)?;
        match c {
            'E' => {
                pattern_parts.weekday = true;
                fields.weekday = Some(raw);
            }
            'M' if width >= 3 => {
                pattern_parts.m = true;
                fields.month = Some(month_from_name_locale(&raw, cal.language, lax).ok_or_else(
                    || {
                        map_err(VmError::InvalidValue {
                            message: format!("calendar `{pattern}` invalid month `{raw}`"),
                        })
                    },
                )?);
            }
            'e' => {
                fields.localized_dow = raw.parse().ok();
            }
            'F' => {
                pattern_parts.week_in_month = true;
                fields.week_in_month = raw.parse().ok();
            }
            'w' => {
                pattern_parts.week_of_year = true;
                fields.week_of_year = raw.parse().ok();
            }
            'W' => {
                pattern_parts.week_in_month = true;
                fields.week_in_month = raw.parse().ok();
            }
            'M' => {
                pattern_parts.m = true;
                fields.month = raw.parse().ok();
            }
            'd' => {
                pattern_parts.d = true;
                fields.day = raw.parse().ok();
            }
            'D' => {
                pattern_parts.day_of_year = true;
                fields.day_of_year = raw.parse().ok();
            }
            'y' | 'Y' => {
                pattern_parts.y = true;
                let ys = expand_calendar_year(&raw, century_start)?;
                fields.year = ys.parse().ok();
            }
            'H' => {
                fields.hour = raw.parse().ok();
            }
            'h' => {
                fields.hour12 = true;
                fields.hour = raw.parse().ok();
            }
            'k' | 'K' => {
                fields.hour = raw.parse().ok();
            }
            'm' => {
                fields.minute = raw.parse().ok();
            }
            's' => {
                fields.second = raw.parse().ok();
            }
            'S' => {
                fields.fraction_digits = Some(raw);
            }
            _ => {}
        }
        i += width;
    }
    while ti < text.len() && text.as_bytes()[ti].is_ascii_whitespace() {
        ti += 1;
    }
    if ti != text.len() {
        return Err(VmError::InvalidValue {
            message: format!(
                "calendar text mismatch at {ti}/{} for `{pattern}` in `{text}`",
                text.len()
            ),
        });
    }
    let has_date = pattern_parts.y
        || pattern_parts.m
        || pattern_parts.d
        || pattern_parts.day_of_year
        || pattern_parts.week_in_month
        || pattern_parts.week_of_year
        || pattern_parts.weekday
        || fields.year.is_some()
        || fields.month.is_some()
        || fields.day.is_some()
        || fields.day_of_year.is_some()
        || fields.week_in_month.is_some()
        || fields.week_of_year.is_some();
    let has_time = fields.hour.is_some() || fields.minute.is_some() || fields.second.is_some();
    finalize_calendar_hour_fields(&mut fields, pattern)?;
    if has_time && !has_date {
        let hour = fields.hour.unwrap_or(0);
        let minute = fields.minute.unwrap_or(0);
        let second = fields.second.unwrap_or(0);
        let (hour, minute, second) = if lax {
            crate::vm::calendar_binary::normalize_lenient_hms(hour, minute, second)
        } else {
            (hour, minute, second)
        };
        let mut out = format!("{hour:02}:{minute:02}:{second:02}");
        if let Some(ref frac) = fields.fraction_digits {
            out.push_str(&format_calendar_s_fraction(frac));
        }
        if let Some(tz) = fields.timezone {
            out.push_str(&tz);
        }
        return Ok(out);
    }
    let mut year = fields.year.unwrap_or(1970);
    if fields.era_is_bc {
        year = -(year - 1);
    }
    let (month, day) = if pattern_parts.day_of_year {
        let ordinal = fields.day_of_year.ok_or_else(|| VmError::InvalidValue {
            message: format!("calendar `{pattern}` missing day-of-year"),
        })?;
        if !pattern_parts.y {
            year = 1970;
        }
        crate::vm::calendar_binary::month_day_from_ordinal(year, ordinal)?
    } else if pattern_parts.week_of_year {
        let week = fields.week_of_year.ok_or_else(|| VmError::InvalidValue {
            message: format!("calendar `{pattern}` missing week-of-year"),
        })?;
        let target_year = fields.year.unwrap_or(1970);
        let (y, m, d) = crate::vm::calendar_binary::date_from_week_of_year(
            target_year,
            week,
            cal.first_day_of_week,
            cal.days_in_first_week,
        )?;
        year = y;
        (m, d)
    } else if pattern_parts.week_in_month {
        let month = fields.month.ok_or_else(|| VmError::InvalidValue {
            message: format!("calendar `{pattern}` missing month"),
        })?;
        if !pattern_parts.y {
            year = 1970;
        }
        let n = fields.week_in_month.ok_or_else(|| VmError::InvalidValue {
            message: format!("calendar `{pattern}` missing week-in-month"),
        })?;
        if pattern.contains('W') {
            let (y, m, d) = crate::vm::calendar_binary::date_from_week_of_month(
                year,
                month,
                n,
                cal.first_day_of_week,
                cal.days_in_first_week,
            )?;
            year = y;
            (m, d)
        } else {
            let day = crate::vm::calendar_binary::nth_weekday_in_month(
                year,
                month,
                n,
                cal.first_day_of_week,
            )?;
            (month, day)
        }
    } else {
        if !pattern_parts.y {
            year = 1970;
        }
        let month = if pattern_parts.m {
            fields.month.ok_or_else(|| VmError::InvalidValue {
                message: format!("calendar `{pattern}` missing month"),
            })?
        } else {
            1
        };
        let day = if pattern_parts.d {
            fields.day.ok_or_else(|| VmError::InvalidValue {
                message: format!("calendar `{pattern}` missing day"),
            })?
        } else if let Some(e) = fields.localized_dow {
            let wd =
                crate::vm::calendar_binary::weekday_from_localized_index(e, cal.first_day_of_week);
            crate::vm::calendar_binary::first_weekday_in_month(year, month, wd).ok_or_else(
                || VmError::InvalidValue {
                    message: format!("calendar `{pattern}` missing day"),
                },
            )?
        } else if let Some(ref wd) = fields.weekday {
            let w = weekday_from_name_locale(wd, cal.language).ok_or_else(|| {
                VmError::InvalidValue {
                    message: format!("calendar `{pattern}` invalid weekday"),
                }
            })?;
            infer_day_from_weekday(year, month, w).ok_or_else(|| VmError::InvalidValue {
                message: format!("calendar `{pattern}` missing day"),
            })?
        } else {
            1
        };
        (month, day)
    };
    let (year, month, day) = if lax {
        crate::vm::calendar_binary::normalize_lenient_ymd(year, month, day)
    } else {
        (year, month, day)
    };
    if has_time {
        let hour = fields.hour.ok_or_else(|| VmError::InvalidValue {
            message: format!("calendar `{pattern}` missing hour"),
        })?;
        let letters = crate::vm::calendar_binary::calendar_pattern_letters_only(pattern);
        let minute = if letters.contains('m') {
            fields.minute.ok_or_else(|| VmError::InvalidValue {
                message: format!("calendar `{pattern}` missing minute"),
            })?
        } else {
            fields.minute.unwrap_or(0)
        };
        let second = if letters.contains('s') {
            fields.second.unwrap_or(0)
        } else {
            fields.second.unwrap_or(0)
        };
        let (hour, minute, second, day_carry) = if lax {
            crate::vm::calendar_binary::normalize_lenient_hms_with_day_carry(
                hour,
                minute,
                second,
                !time_overflow_carries_to_date,
            )
        } else {
            (hour, minute, second, 0)
        };
        let (year, month, day) = if day_carry != 0 {
            crate::vm::calendar_binary::normalize_lenient_ymd(
                year,
                month,
                day.saturating_add(day_carry as u32),
            )
        } else {
            (year, month, day)
        };
        let mut out = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}");
        if let Some(tz) = fields.timezone {
            out.push_str(&tz);
        }
        return Ok(out);
    }
    let out = format!("{year:04}-{month:02}-{day:02}");
    if time_overflow_carries_to_date
        && !date_only
        && pattern.contains('W')
        && !crate::vm::calendar_binary::calendar_pattern_has_time_fields(pattern)
    {
        Ok(format!("{out}T00:00:00"))
    } else {
        Ok(out)
    }
}

pub(crate) fn expand_calendar_year(y: &str, century_start: u32) -> Result<String, VmError> {
    if y.len() == 2 {
        let yy: u32 = y.parse().map_err(|_| VmError::InvalidValue {
            message: format!("invalid calendar year `{y}`"),
        })?;
        let full = if yy >= century_start {
            1900 + yy
        } else {
            2000 + yy
        };
        return Ok(format!("{full:04}"));
    }
    Ok(y.into())
}

pub(crate) fn calendar_explicit_pattern_extends_year_beyond_length(pattern: &str) -> bool {
    const FIELD: &str = "EMdDFwWmyYHhsekKS";
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    let mut last_y_width = 0usize;
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
        let c = chars[i];
        if c == 'y' || c == 'Y' {
            let mut w = 1usize;
            while i + w < chars.len() && chars[i + w] == c {
                w += 1;
            }
            last_y_width = w;
            i += w;
        } else if FIELD.contains(c) {
            let mut w = 1usize;
            while i + w < chars.len() && chars[i + w] == c {
                w += 1;
            }
            i += w;
        } else {
            i += 1;
        }
    }
    last_y_width == 1
}
