//! Standard text number parsing (`textNumberRep="standard"`) using `textNumberPattern`.
//!
//! Lax/strict behavior follows Daffodil/ICU `DecimalFormat.setParseStrict`.

use crate::error::VmError;
use crate::schema::BinaryNumberCheckPolicy;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone)]
pub(crate) struct TextNumberFormatProps<'a> {
    pub check_policy: BinaryNumberCheckPolicy,
    pub decimal_separators: &'a [String],
    pub grouping_separator: Option<&'a str>,
    pub exponent_chars: &'a str,
    pub pad_character: Option<char>,
    pub ignore_case: bool,
}

impl Default for TextNumberFormatProps<'_> {
    fn default() -> Self {
        Self {
            check_policy: BinaryNumberCheckPolicy::Lax,
            decimal_separators: &[],
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ignore_case: false,
        }
    }
}

fn slice_starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.as_bytes()[..prefix.len()]
            .eq_ignore_ascii_case(prefix.as_bytes())
}

fn default_decimal_separators() -> Vec<String> {
    vec![".".into()]
}

/// Parse data against a DFDL text number pattern; returns a canonical numeric string for `parse()` / `parse_float`.
pub(crate) fn pattern_without_quoted_regions(pattern: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn digit_slots_before_v(pattern: &str) -> usize {
    let bare = pattern_without_quoted_regions(pattern);
    let Some(v_idx) = bare.find('V') else {
        return 0;
    };
    bare[..v_idx]
        .chars()
        .filter(|c| matches!(c, '0' | '#'))
        .count()
}

fn v_fraction_digit_count(pattern: &str) -> usize {
    let bare = pattern_without_quoted_regions(pattern);
    let Some(v_idx) = bare.find('V') else {
        return 0;
    };
    bare[v_idx + 1..]
        .chars()
        .filter(|c| matches!(c, '0' | '#'))
        .count()
}

fn strip_v_from_pattern(pattern: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            out.push('\'');
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if chars[i] != 'V' {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

/// Daffodil `P` (decimal scaling position) virtual point from the positive subpattern.
fn text_decimal_virtual_point_from_pattern(pattern: &str) -> i32 {
    let bare = pattern_without_quoted_regions(pattern);
    if bare.contains('V') {
        return 0;
    }
    if !bare.contains('P') {
        return 0;
    }
    let chars: Vec<char> = bare.chars().collect();
    let mut p_left = 0usize;
    let mut digits_left = 0usize;
    let mut p_right = 0usize;
    let mut phase = 0u8; // 0=prefix, 1=Ps left, 2=digits, 3=Ps right
    for c in chars {
        match phase {
            0 if c == 'P' => {
                phase = 1;
                p_left += 1;
            }
            0 if matches!(c, '0' | '#' | '*' | '.' | ',' | 'E' | 'e' | ' ') => {
                phase = 2;
                if matches!(c, '0' | '#') {
                    digits_left += 1;
                }
            }
            1 if c == 'P' => p_left += 1,
            1 if matches!(c, '0' | '#') => {
                phase = 2;
                digits_left += 1;
            }
            2 if c == 'P' => {
                phase = 3;
                p_right += 1;
            }
            2 if matches!(c, '0' | '#') => digits_left += 1,
            3 if c == 'P' => p_right += 1,
            _ => {}
        }
    }
    if p_left > 0 && p_right == 0 {
        (p_left + digits_left) as i32
    } else if p_right > 0 && p_left == 0 {
        -(p_right as i32)
    } else {
        0
    }
}

fn strip_p_from_pattern(pattern: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            out.push('\'');
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if chars[i] != 'P' {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

fn apply_decimal_virtual_point(num: &str, vp: i32) -> String {
    if vp == 0 {
        return num.to_string();
    }
    let negative = num.starts_with('-');
    let body = num.trim_start_matches('-');
    let (mantissa, exp_suffix) = if let Some(idx) = body.find('E').or_else(|| body.find('e')) {
        (&body[..idx], Some(&body[idx..]))
    } else {
        (body, None)
    };
    let (int_part, frac_part) = if let Some(dot) = mantissa.find('.') {
        (&mantissa[..dot], &mantissa[dot + 1..])
    } else {
        (mantissa, "")
    };
    let mut digits: String = int_part.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.extend(frac_part.chars().filter(|c| c.is_ascii_digit()));
    if digits.is_empty() {
        digits.push('0');
    }
    let scaled = if vp > 0 {
        let vp = vp as usize;
        if digits.len() <= vp {
            let zeros = vp - digits.len();
            format!("0.{}{}", "0".repeat(zeros), digits.trim_end_matches('0'))
        } else {
            let split = digits.len() - vp;
            let (a, b) = digits.split_at(split);
            let frac = b.trim_end_matches('0');
            if frac.is_empty() {
                a.to_string()
            } else {
                format!("{a}.{frac}")
            }
        }
    } else {
        let mul = (-vp) as usize;
        let trimmed = digits.trim_start_matches('0');
        let core = if trimmed.is_empty() { "0" } else { trimmed };
        format!("{core}{}", "0".repeat(mul))
    };
    let mut out = if negative {
        format!("-{scaled}")
    } else {
        scaled
    };
    if let Some(exp) = exp_suffix {
        out.push_str(exp);
    }
    out
}

fn strip_unquoted_char(s: &str, ch: char) -> String {
    let mut out = String::new();
    let mut in_quote = false;
    for c in s.chars() {
        if c == '\'' {
            in_quote = !in_quote;
            out.push(c);
            continue;
        }
        if in_quote || c != ch {
            out.push(c);
        }
    }
    out
}

fn strip_unquoted_substr(s: &str, sub: &str) -> String {
    if sub.len() == 1 {
        return strip_unquoted_char(s, sub.chars().next().unwrap());
    }
    let mut out = String::new();
    let mut in_quote = false;
    let chars: Vec<char> = s.chars().collect();
    let sub_chars: Vec<char> = sub.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            in_quote = !in_quote;
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if !in_quote && i + sub_chars.len() <= chars.len() && chars[i..i + sub_chars.len()] == sub_chars[..] {
            i += sub_chars.len();
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// XSD attribute values encode a pattern apostrophe as `''`; normalize to one `'` outside ICU quotes.
fn expand_pattern_doubled_apostrophes(pattern: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    let mut in_quote = false;
    while i < chars.len() {
        if chars[i] == '\'' {
            if in_quote {
                if i + 1 < chars.len() && chars[i + 1] == '\'' {
                    out.push('\'');
                    i += 2;
                    continue;
                }
                in_quote = false;
                out.push('\'');
                i += 1;
                continue;
            }
            if i + 1 < chars.len() && chars[i + 1] == '\'' {
                out.push('\'');
                i += 2;
                continue;
            }
            in_quote = true;
            out.push('\'');
            i += 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Shortest prefix of `data` that fully matches `pattern` under `props`, if any.
pub(crate) fn implicit_text_number_byte_length(
    data: &[u8],
    pattern: &str,
    props: &TextNumberFormatProps<'_>,
) -> Option<usize> {
    let mut best = None;
    for end in 1..=data.len() {
        let Ok(text) = core::str::from_utf8(&data[..end]) else {
            continue;
        };
        if parse_standard_text_number(text, pattern, props).is_ok() {
            best = Some(end);
        }
    }
    best
}

fn apply_configured_separators_to_input(
    work: &str,
    pattern: &str,
    props: &TextNumberFormatProps<'_>,
) -> String {
    let bare = pattern_without_quoted_regions(pattern);
    let mut s = work.to_string();
    if let Some(grp) = props.grouping_separator {
        if !grp.is_empty() && (grp != "," || !bare.contains(',')) {
            s = strip_unquoted_substr(&s, grp);
        }
    }
    if !bare.contains('.') {
        for ds in props.decimal_separators {
            if !ds.is_empty() {
                if let Some(idx) = s.find(ds.as_str()) {
                    s.replace_range(idx..idx + ds.len(), ".");
                    break;
                }
            }
        }
    }
    s
}

fn needs_separator_preprocess(work: &str, pattern: &str, props: &TextNumberFormatProps<'_>) -> bool {
    let bare = pattern_without_quoted_regions(pattern);
    if props
        .grouping_separator
        .is_some_and(|g| !g.is_empty() && g != "," && work.contains(g))
    {
        return true;
    }
    if !bare.contains('.') {
        return props.decimal_separators.iter().any(|ds| {
            !ds.is_empty() && ds != "." && work.contains(ds.as_str())
        });
    }
    false
}

pub(crate) fn parse_standard_text_number(
    input: &str,
    pattern: &str,
    props: &TextNumberFormatProps<'_>,
) -> Result<String, VmError> {
    let lax = props.check_policy == BinaryNumberCheckPolicy::Lax;
    let mut work = if lax {
        input.trim().to_string()
    } else {
        input.to_string()
    };

    let pattern = expand_pattern_doubled_apostrophes(pattern);
    let pattern = pattern.as_str();

    let custom_grouping = props
        .grouping_separator
        .map(|g| !g.is_empty() && g != "," && work.contains(g))
        .unwrap_or(false);
    if needs_separator_preprocess(&work, pattern, props) {
        work = apply_configured_separators_to_input(&work, pattern, props);
    }

    let pattern_has_grouping = strip_unquoted_char(pattern, ',').len() < pattern.len();
    let int_comma_count = pattern_without_quoted_regions(pattern)
        .split(['.', 'E', 'e', ';'])
        .next()
        .unwrap_or("")
        .chars()
        .filter(|&c| c == ',')
        .count();
    let relax_grouping = lax || int_comma_count >= 3 || custom_grouping;
    let (work, pattern_owned) = if pattern_has_grouping && relax_grouping {
        if let Some(grp) = props.grouping_separator {
            if !grp.is_empty() {
                (
                    strip_unquoted_substr(&work, grp),
                    strip_unquoted_char(pattern, ','),
                )
            } else {
                (work, pattern.to_string())
            }
        } else {
            (work, pattern.to_string())
        }
    } else {
        (work, pattern.to_string())
    };
    let pattern = pattern_owned.as_str();

    let subpatterns: Vec<&str> = if pattern.contains(';') {
        pattern.split(';').collect()
    } else {
        vec![pattern]
    };

    let positive = subpatterns.first().copied().unwrap_or(pattern);
    let v_frac = v_fraction_digit_count(positive);
    let v_int_slots = digit_slots_before_v(positive);
    let p_virtual = if v_frac > 0 {
        0
    } else {
        text_decimal_virtual_point_from_pattern(positive)
    };
    let match_pattern = if v_frac > 0 {
        strip_v_from_pattern(pattern)
    } else if p_virtual != 0 {
        strip_p_from_pattern(pattern)
    } else {
        pattern.to_string()
    };
    let match_subs: Vec<&str> = if match_pattern.contains(';') {
        match_pattern.split(';').collect()
    } else {
        vec![match_pattern.as_str()]
    };
    let positive_match = match_subs.first().copied().unwrap_or(match_pattern.as_str());

    for (idx, sub) in match_subs.iter().enumerate() {
        if idx > 0 && sub.is_empty() {
            continue;
        }
        let mut scratch = work.clone();
        let result = if idx > 0 {
            match_negative_subpattern(
                &mut scratch,
                sub,
                positive_match,
                props,
                lax,
                v_frac,
                v_int_slots,
            )
        } else {
            match_subpattern(
                &mut scratch,
                sub,
                props,
                lax,
                false,
                v_frac,
                v_int_slots,
            )
        };
        match result {
            Ok(num) => {
                if p_virtual != 0 {
                    return Ok(apply_decimal_virtual_point(&num, p_virtual));
                }
                return Ok(num);
            }
            Err(_) => continue,
        }
    }

    // Optional negative subpattern (pattern ends with ';' only): negate positive match.
    if subpatterns.len() >= 2
        && subpatterns.get(1).is_some_and(|s| s.is_empty())
        && work.starts_with('-')
    {
        let mut scratch = work[1..].to_string();
        if let Ok(num) = match_subpattern(
            &mut scratch,
            positive_match,
            props,
            lax,
            false,
            v_frac,
            v_int_slots,
        ) {
            let out = if p_virtual != 0 {
                apply_decimal_virtual_point(&num, p_virtual)
            } else {
                num
            };
            return Ok(if out.starts_with('-') {
                out
            } else {
                format!("-{out}")
            });
        }
    }

    Err(VmError::InvalidValue {
        message: format!("Parse Error. Unable to parse number from text: {input}"),
    })
}

fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\n' || b == b'\r'
}

fn skip_ws(bytes: &[u8], pos: &mut usize, lax: bool) {
    if !lax {
        return;
    }
    while *pos < bytes.len() && is_ws(bytes[*pos]) {
        *pos += 1;
    }
}

/// Lax whitespace skip that must not consume a configured decimal separator (e.g. `%SP;`).
fn skip_ws_respecting_decimal_sep(
    bytes: &[u8],
    pos: &mut usize,
    props: &TextNumberFormatProps<'_>,
) {
    while *pos < bytes.len() && is_ws(bytes[*pos]) {
        if props
            .decimal_separators
            .iter()
            .any(|ds| bytes[*pos..].starts_with(ds.as_bytes()))
        {
            break;
        }
        *pos += 1;
    }
}

fn match_decimal_separator(
    bytes: &[u8],
    pos: &mut usize,
    props: &TextNumberFormatProps<'_>,
) -> bool {
    for ds in props.decimal_separators {
        if *pos + ds.len() <= bytes.len() && bytes[*pos..].starts_with(ds.as_bytes()) {
            *pos += ds.len();
            return true;
        }
    }
    if props.decimal_separators.is_empty()
        && *pos < bytes.len()
        && bytes[*pos] == b'.'
    {
        *pos += 1;
        return true;
    }
    false
}

fn is_pattern_digit_slot(c: char) -> bool {
    matches!(c, '0' | '#' | '*' | '.' | ',' | 'E' | 'e' | 'V' | 'P' | ' ')
}

fn is_negative_template_char(c: char) -> bool {
    matches!(
        c,
        '0' | '#' | '.' | ',' | 'E' | 'e' | 'V' | 'P' | ' ' | '+' | '-'
    )
}

fn fuzzy_subpattern_slot_char(c: char) -> bool {
    matches!(c, '0' | '#' | '.' | ',' | 'E' | 'e' | 'V' | 'P' | ' ')
}

/// Digit/pad template region in a negative subpattern (digit specs are ignored for parsing).
fn negative_template_span(pattern: &str) -> (usize, usize) {
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            if let Some((_, ni)) = parse_quoted_pattern_literal(&chars, i) {
                i = ni;
                continue;
            }
            break;
        }
        if chars[i] == '*' {
            if i + 1 < chars.len() {
                i += 2;
                continue;
            }
        }
        if is_negative_template_char(chars[i]) {
            break;
        }
        i += 1;
    }
    let start = i;
    while i < chars.len() {
        if chars[i] == '\'' {
            break;
        }
        if chars[i] == '*' {
            if i + 1 < chars.len() {
                i += 2;
                continue;
            }
        }
        if chars[i] == ' ' {
            let mut j = i;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            if j < chars.len() && chars[j] == '\'' {
                break;
            }
        }
        if is_negative_template_char(chars[i]) {
            i += 1;
            continue;
        }
        break;
    }
    (start, i)
}

fn pattern_literals_between(chars: &[char], start: usize, end: usize) -> String {
    let mut out = String::new();
    let mut i = start;
    while i < end {
        if chars[i] == '\'' {
            if let Some((lit, ni)) = parse_quoted_pattern_literal(chars, i) {
                out.push_str(&lit);
                i = ni;
                continue;
            }
            i += 1;
            continue;
        }
        if chars[i] == '*' && i + 1 < end {
            i += 2;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn fuzzy_positive_in_negative(negative: &str, positive: &str) -> Option<(usize, usize)> {
    let neg: Vec<char> = negative.chars().collect();
    let pos: Vec<char> = positive.chars().collect();
    if pos.is_empty() {
        return None;
    }
    for start in 0..neg.len() {
        let mut ni = start;
        let mut pi = 0usize;
        while pi < pos.len() && ni < neg.len() {
            if neg[ni] == '\'' {
                if let Some((lit, nnext)) = parse_quoted_pattern_literal(&neg, ni) {
                    if pos[pi] != '\'' {
                        break;
                    }
                    if let Some((plit, pnext)) = parse_quoted_pattern_literal(&pos, pi) {
                        if lit != plit {
                            break;
                        }
                        ni = nnext;
                        pi = pnext;
                        continue;
                    }
                    break;
                }
                break;
            }
            if pos[pi] == '\'' {
                break;
            }
            if neg[ni] == '*' && pi + 1 < pos.len() && pos[pi] == '*' {
                ni += 2;
                pi += 2;
                continue;
            }
            if fuzzy_subpattern_slot_char(pos[pi]) && fuzzy_subpattern_slot_char(neg[ni]) {
                ni += 1;
                pi += 1;
                continue;
            }
            if neg[ni] == pos[pi] {
                ni += 1;
                pi += 1;
                continue;
            }
            break;
        }
        if pi == pos.len() {
            return Some((start, ni));
        }
        // Sign suffix affixes on negative subpatterns (e.g. `##0PP+` / `##0PP-`).
        if pi + 1 == pos.len()
            && ni + 1 == neg.len()
            && matches!(pos[pi], '+' | '-')
            && matches!(neg[ni], '+' | '-')
        {
            return Some((start, ni));
        }
    }
    None
}

fn negative_affixes(negative_pattern: &str, positive_pattern: &str) -> (String, String) {
    if let Some((start, end)) = fuzzy_positive_in_negative(negative_pattern, positive_pattern) {
        let chars: Vec<char> = negative_pattern.chars().collect();
        let prefix = pattern_literals_between(&chars, 0, start);
        let suffix = pattern_literals_between(&chars, end, chars.len());
        return (prefix, suffix);
    }
    let chars: Vec<char> = negative_pattern.chars().collect();
    let (tmpl_start, tmpl_end) = negative_template_span(negative_pattern);
    let prefix = pattern_literals_between(&chars, 0, tmpl_start);
    let suffix = pattern_literals_between(&chars, tmpl_end, chars.len());
    (prefix, suffix)
}

fn pad_char_from_positive_pattern(pattern: &str) -> Option<char> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i + 1 < chars.len() {
        if chars[i] == '*' && chars[i + 1] != '\'' {
            return Some(chars[i + 1]);
        }
        if is_negative_template_char(chars[i]) || chars[i] == '#' {
            break;
        }
        if chars[i] == '\'' {
            if let Some((_, ni)) = parse_quoted_pattern_literal(&chars, i) {
                i = ni;
                continue;
            }
            break;
        }
        i += 1;
    }
    None
}

fn match_negative_prefix(
    text: &str,
    pos: &mut usize,
    prefix: &str,
    positive_pattern: &str,
    lax: bool,
) -> Result<(), VmError> {
    if prefix.is_empty() {
        return Ok(());
    }
    skip_ws(text.as_bytes(), pos, lax);
    if text[*pos..].starts_with(prefix) {
        *pos += prefix.len();
        return Ok(());
    }
    let pad = pad_char_from_positive_pattern(positive_pattern);
    if let Some(pad_ch) = pad {
        let mut p = *pos;
        while p < text.len() {
            let b = text.as_bytes()[p];
            if b == pad_ch as u8 {
                p += 1;
                continue;
            }
            break;
        }
        if text[p..].starts_with(prefix) {
            *pos = p + prefix.len();
            return Ok(());
        }
    }
    Err(VmError::InvalidValue {
        message: "textNumberPattern mismatch".into(),
    })
}

fn trailing_star_pad_from_negative_pattern(pattern: &str) -> Option<char> {
    let chars: Vec<char> = pattern.chars().collect();
    if chars.len() < 2 {
        return None;
    }
    let n = chars.len();
    if chars[n - 2] == '*' {
        Some(chars[n - 1])
    } else {
        None
    }
}

fn match_negative_subpattern(
    text: &mut String,
    negative_pattern: &str,
    positive_pattern: &str,
    props: &TextNumberFormatProps<'_>,
    lax: bool,
    v_frac_digits: usize,
    v_int_slots: usize,
) -> Result<String, VmError> {
    let (prefix, suffix) = negative_affixes(negative_pattern, positive_pattern);
    let neg_trailing_pad = trailing_star_pad_from_negative_pattern(negative_pattern);
    let bytes = text.as_bytes();
    let mut pos = 0usize;
    skip_ws(bytes, &mut pos, lax);
    match_negative_prefix(text, &mut pos, &prefix, positive_pattern, lax)?;
    let mut end = bytes.len();
    if lax {
        while end > pos && is_ws(bytes[end - 1]) {
            end -= 1;
        }
    }
    if !suffix.is_empty() {
        let sl = suffix.len();
        if end >= sl && &text[end - sl..end] == suffix.as_str() {
            end -= sl;
        } else if let Some(rel) = text[pos..end].find(suffix.as_str()) {
            end = pos + rel;
        } else {
            return Err(VmError::InvalidValue {
                message: "textNumberPattern mismatch".into(),
            });
        }
    }
    if pos > end {
        return Err(VmError::InvalidValue {
            message: "textNumberPattern mismatch".into(),
        });
    }
    let core = text[pos..end].to_string();
    let mut inner = core;
    let result = match_subpattern(
        &mut inner,
        positive_pattern,
        props,
        lax,
        true,
        v_frac_digits,
        v_int_slots,
    )?;
    let mut tail = end;
    if !suffix.is_empty() {
        if !text[tail..].starts_with(suffix.as_str()) {
            return Err(VmError::InvalidValue {
                message: "textNumberPattern mismatch".into(),
            });
        }
        tail += suffix.len();
    }
    if let Some(pad) = neg_trailing_pad {
        while tail < text.len() && text.as_bytes()[tail] == pad as u8 {
            tail += 1;
        }
    }
    if lax {
        skip_ws(bytes, &mut tail, true);
    }
    if tail != text.len() {
        return Err(VmError::InvalidValue {
            message: "textNumberPattern mismatch".into(),
        });
    }
    Ok(result)
}

fn skip_pad_chars(
    bytes: &[u8],
    pos: &mut usize,
    pad: char,
    pattern_pad: bool,
    lax: bool,
    props: &TextNumberFormatProps<'_>,
    after_decimal: bool,
) {
    if !lax && !pattern_pad {
        return;
    }
    loop {
        // Do not skip generic whitespace when consuming pattern pad (`*x`); that space may be
        // a required literal before a suffix (e.g. `#*> right`).
        if !pattern_pad {
            skip_ws_respecting_decimal_sep(bytes, pos, props);
        }
        if *pos >= bytes.len() {
            break;
        }
        let ch = bytes[*pos] as char;
        if Some(ch) == Some(pad) {
            *pos += ch.len_utf8();
            continue;
        }
        if !after_decimal && pad == '0' && bytes[*pos] == b'0' {
            *pos += 1;
            continue;
        }
        break;
    }
}

fn skip_pad(
    bytes: &[u8],
    pos: &mut usize,
    pad: Option<char>,
    lax: bool,
    props: &TextNumberFormatProps<'_>,
    after_decimal: bool,
) {
    if let Some(p) = pad {
        skip_pad_chars(bytes, pos, p, false, lax, props, after_decimal);
    }
}

fn parse_quoted_pattern_literal(chars: &[char], i: usize) -> Option<(String, usize)> {
    if chars.get(i) != Some(&'\'') {
        return None;
    }
    let mut j = i + 1;
    let mut lit = String::new();
    while j < chars.len() {
        if chars[j] == '\'' {
            if j + 1 < chars.len() && chars[j + 1] == '\'' {
                lit.push('\'');
                j += 2;
                continue;
            }
            return Some((lit, j + 1));
        }
        lit.push(chars[j]);
        j += 1;
    }
    None
}

fn match_subpattern(
    text: &mut String,
    pattern: &str,
    props: &TextNumberFormatProps<'_>,
    lax: bool,
    negative_subpattern: bool,
    v_frac_digits: usize,
    v_int_slots: usize,
) -> Result<String, VmError> {
    let bare_pattern = pattern_without_quoted_regions(pattern);
    let bytes = text.as_bytes();
    let mut pos = 0usize;
    if lax && !negative_subpattern && pos < bytes.len() && bytes[pos] == b'+' {
        pos += 1;
    }
    let mut int_digits = String::new();
    let mut frac_digits = String::new();
    let mut exponent: Option<String> = None;
    let mut exp_negative = false;
    let mut negative = negative_subpattern;
    let mut saw_decimal = false;
    let mut in_exponent = false;
    let mut saw_digit = false;
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            if let Some((lit, new_i)) = parse_quoted_pattern_literal(&chars, i) {
                i = new_i;
                skip_ws(bytes, &mut pos, lax);
                if text[pos..].starts_with(&lit) {
                    pos += lit.len();
                } else if lax || negative {
                    // lax: quoted literal may be omitted (DFDL-13-052R); also when parsing
                    // the numeric core after a negative subpattern prefix (hex sign C/D, etc.)
                } else {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                continue;
            }
            skip_ws(bytes, &mut pos, lax);
            if pos < bytes.len() && bytes[pos] == b'\'' {
                pos += 1;
                i += 1;
                continue;
            }
            if !lax {
                return Err(VmError::InvalidValue {
                    message: "textNumberPattern mismatch".into(),
                });
            }
            i += 1;
            continue;
        }

        match chars[i] {
            '0' | '#' => {
                let mut j = i;
                let mut max_digits = 0usize;
                while j < chars.len() && matches!(chars[j], '0' | '#') {
                    max_digits += 1;
                    j += 1;
                }
                i = j;
                // DFDL/ICU parse: digit characters in the data are always recognized (0 vs # affects unparse only).
                let min_digits = 0usize;
                let stops_at_dot = j < chars.len() && chars[j] == '.';

                skip_pad(bytes, &mut pos, props.pad_character, lax, props, saw_decimal);
                if in_exponent && exponent.as_ref().map_or(true, |e| e.is_empty()) {
                    if pos < bytes.len() && bytes[pos] == b'+' {
                        pos += 1;
                    } else if pos < bytes.len() && bytes[pos] == b'-' {
                        exp_negative = true;
                        pos += 1;
                    }
                }
                if !in_exponent && !saw_decimal {
                    if match_decimal_separator(bytes, &mut pos, props) {
                        saw_decimal = true;
                    }
                }
                let start = pos;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    if stops_at_dot && !saw_decimal && !in_exponent {
                        if match_decimal_separator(bytes, &mut pos, props) {
                            saw_decimal = true;
                            break;
                        }
                    }
                    pos += 1;
                }
                let digit_count = pos - start;
                if digit_count > 0 {
                    saw_digit = true;
                }
                if digit_count < min_digits && !lax {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                if digit_count == 0 && min_digits > 0 && !lax {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                let chunk = core::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                if in_exponent {
                    exponent.get_or_insert_with(String::new).push_str(chunk);
                } else if saw_decimal {
                    frac_digits.push_str(chunk);
                } else {
                    int_digits.push_str(chunk);
                }
            }
            '*' => {
                if i + 1 >= chars.len() {
                    return Err(VmError::InvalidValue {
                        message: format!("invalid textNumberPattern `{pattern}`"),
                    });
                }
                let esc_pad = chars[i + 1];
                i += 2;
                skip_pad_chars(bytes, &mut pos, esc_pad, true, lax, props, saw_decimal);
            }
            '.' => {
                // Match decimal separator before lax whitespace skip — when the separator is
                // space (or other WS), skip_ws would consume it and break parsing (DFDL-13-053R).
                let mut matched = match_decimal_separator(bytes, &mut pos, props);
                if !matched && lax {
                    skip_ws_respecting_decimal_sep(bytes, &mut pos, props);
                    matched = match_decimal_separator(bytes, &mut pos, props);
                }
                if !matched && !lax {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                if matched {
                    saw_decimal = true;
                }
                i += 1;
            }
            ',' => {
                skip_ws(bytes, &mut pos, lax);
                if let Some(grp) = props.grouping_separator {
                    if !grp.is_empty() && text[pos..].starts_with(grp) {
                        pos += grp.len();
                    } else if !lax {
                        return Err(VmError::InvalidValue {
                            message: "textNumberPattern mismatch".into(),
                        });
                    }
                }
                i += 1;
            }
            'E' | 'e' => {
                skip_ws(bytes, &mut pos, lax);
                if props.exponent_chars.is_empty() {
                    in_exponent = true;
                    i += 1;
                    continue;
                }
                if !props.exponent_chars.is_empty() {
                    let matched = if props.ignore_case {
                        slice_starts_with_ignore_case(&text[pos..], props.exponent_chars)
                    } else {
                        text[pos..].starts_with(props.exponent_chars)
                    };
                    if matched {
                        pos += props.exponent_chars.len();
                        in_exponent = true;
                        i += 1;
                        continue;
                    }
                }
                if props.exponent_chars.contains(chars[i]) {
                    if pos < bytes.len() {
                        let b = bytes[pos];
                        if b == b'E' || b == b'e' || props.exponent_chars.contains(b as char) {
                            pos += 1;
                            in_exponent = true;
                            i += 1;
                            continue;
                        }
                    }
                }
                // fall through literal
                let lit = chars[i];
                skip_ws(bytes, &mut pos, lax);
                if pos + lit.len_utf8() <= bytes.len() {
                    let slice = core::str::from_utf8(&bytes[pos..pos + lit.len_utf8()]).ok();
                    if slice == Some(lit.to_string().as_str()) {
                        pos += lit.len_utf8();
                        i += 1;
                        continue;
                    }
                }
                if !lax {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                i += 1;
            }
            ' ' => {
                if lax {
                    skip_ws(bytes, &mut pos, true);
                } else if pos < bytes.len() && is_ws(bytes[pos]) {
                    pos += 1;
                } else {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                i += 1;
            }
            'V' | 'P' => {
                // virtual decimal point / scaling position — not present in data
                i += 1;
            }
            ';' => {
                i += 1;
            }
            other => {
                skip_ws(bytes, &mut pos, lax);
                let lit = other;
                if in_exponent && lit == '+' {
                    if text[pos..].starts_with('+') {
                        pos += 1;
                    } else if pos < bytes.len() && bytes[pos] == b'-' {
                        // optional '+' omitted when exponent is negative
                    } else if !lax {
                        return Err(VmError::InvalidValue {
                            message: "textNumberPattern mismatch".into(),
                        });
                    }
                    i += 1;
                    continue;
                }
                if pos + lit.len_utf8() <= bytes.len() {
                    let slice = core::str::from_utf8(&bytes[pos..pos + lit.len_utf8()]).ok();
                    if slice == Some(lit.to_string().as_str()) {
                        pos += lit.len_utf8();
                        i += 1;
                        continue;
                    }
                }
                if !lax {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                i += 1;
            }
        }
    }

    if !saw_decimal
        && !bare_pattern.contains('.')
        && pos < bytes.len()
        && (bytes[pos] == b'.'
            || props
                .decimal_separators
                .iter()
                .any(|ds| bytes[pos..].starts_with(ds.as_bytes())))
    {
        if bytes[pos] == b'.' {
            pos += 1;
        } else {
            let _ = match_decimal_separator(bytes, &mut pos, props);
        }
        saw_decimal = true;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            frac_digits.push(bytes[pos] as char);
            pos += 1;
        }
    }

    skip_ws(bytes, &mut pos, lax);
    if pos != bytes.len() {
        if lax {
            // allow trailing junk only if all digits consumed — strict about leftover
            let rest = core::str::from_utf8(&bytes[pos..]).unwrap_or("");
            if rest.chars().any(|c| c.is_ascii_digit()) {
                return Err(VmError::InvalidValue {
                    message: "textNumberPattern mismatch".into(),
                });
            }
            // Trailing +/- may belong to an alternate (negative) subpattern.
            if rest == "-" || rest == "+" {
                return Err(VmError::InvalidValue {
                    message: "textNumberPattern mismatch".into(),
                });
            }
        } else {
            return Err(VmError::InvalidValue {
                message: "textNumberPattern mismatch".into(),
            });
        }
    }

    if v_frac_digits > 0 && frac_digits.is_empty() {
        let total_slots = v_int_slots + v_frac_digits;
        if lax && total_slots > 0 && int_digits.len() < total_slots {
            let pad_len = total_slots - int_digits.len();
            int_digits.insert_str(0, &"0".repeat(pad_len));
        }
        if int_digits.len() >= v_frac_digits {
            let split_at = int_digits.len() - v_frac_digits;
            frac_digits = int_digits[split_at..].to_string();
            int_digits.truncate(split_at);
            saw_decimal = true;
        }
    }

    let int_norm = normalize_int_digits(&int_digits);
    let mut out = if saw_decimal || !frac_digits.is_empty() {
        if frac_digits.is_empty() {
            int_norm.clone()
        } else {
            format!("{}.{}", int_norm, frac_digits)
        }
    } else {
        int_norm
    };

    if let Some(exp) = exponent {
        if !exp.is_empty() {
            let bare = pattern_without_quoted_regions(pattern);
            if saw_decimal
                || !frac_digits.is_empty()
                || bare.contains('E')
                || bare.contains('e')
            {
                let exp_sign = if exp_negative { "-" } else { "" };
                let mut sci = format!("{out}E{exp_sign}{exp}");
                if negative && !sci.starts_with('-') {
                    sci.insert(0, '-');
                }
                return Ok(sci);
            }
            return Ok(apply_scientific_exponent(&out, &exp, exp_negative, negative));
        }
    }

    if negative && !out.starts_with('-') {
        out.insert(0, '-');
    }

    if !saw_digit && pattern.contains('*') {
        return Err(VmError::InvalidValue {
            message: "textNumberPattern mismatch".into(),
        });
    }

    Ok(out)
}

fn apply_scientific_exponent(
    mantissa: &str,
    exp: &str,
    exp_negative: bool,
    mantissa_negative: bool,
) -> String {
    let e: i32 = exp.parse().unwrap_or(0);
    let power = if exp_negative { -e } else { e };
    let m: f64 = mantissa.parse().unwrap_or(0.0);
    let signed_m = if mantissa_negative { -m } else { m };
    let mut scale = 1.0f64;
    let steps = power.unsigned_abs();
    for _ in 0..steps {
        scale *= 10.0;
    }
    let v = if power >= 0 {
        signed_m * scale
    } else {
        signed_m / scale
    };
    let mut s = format!("{v}");
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

fn grouping_segment_slot_counts(pattern: &str) -> Vec<usize> {
    let bare = pattern_without_quoted_regions(pattern);
    let int_part = bare
        .split(['.', 'E', 'e', ';'])
        .next()
        .unwrap_or(bare.as_str());
    int_part
        .split(',')
        .map(|seg| seg.chars().filter(|c| matches!(c, '#' | '0')).count())
        .filter(|&n| n > 0)
        .collect()
}

fn normalize_int_digits(digits: &str) -> String {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".into()
    } else {
        trimmed.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lax_pattern_with_spaces() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            decimal_separators: &dec,
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("           0052    ", "    0000    ", &props).unwrap();
        assert_eq!(n, "52");
    }

    #[test]
    fn exponent_pattern() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            decimal_separators: &dec,
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("006.54E9", "000.0#E0", &props).unwrap();
        assert_eq!(n, "6.54E9");
    }

    #[test]
    fn strict_pad_exponent_pattern() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("006.54E9", "000.0#E0", &props).unwrap();
        assert_eq!(n, "6.54E9");
    }

    #[test]
    fn lax_optional_quoted_prefix() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            decimal_separators: &dec,
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("1234", "'$'0000", &props).unwrap();
        assert_eq!(n, "1234");
        let n2 = parse_standard_text_number("1234", "'optional:'0000", &props).unwrap();
        assert_eq!(n2, "1234");
    }

    #[test]
    fn lax_space_decimal_separator() {
        let dec = vec![" ".into()];
        let lax_props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Lax,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let strict_props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let strict =
            parse_standard_text_number("$5 00", "'$'#0.00", &strict_props).unwrap();
        assert_eq!(strict, "5.00", "strict baseline got `{strict}`");
        let n = parse_standard_text_number("$5 00", "'$'#0.00", &lax_props).unwrap();
        assert_eq!(n, "5.00", "lax parse got `{n}` (strict=`{strict}`)");
        let n2 = parse_standard_text_number("$5 50", "'$'#0.00", &lax_props).unwrap();
        assert_eq!(n2, "5.50", "lax parse got `{n2}`");
    }

    #[test]
    fn space_decimal_separator() {
        let dec = vec![" ".into()];
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("$5 00", "'$'#0.00", &props).unwrap();
        assert_eq!(n, "5.00");
    }

    #[test]
    fn quoted_dollar_float() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("$49.99", "'$'##0.00", &props).unwrap();
        assert_eq!(n, "49.99");
    }

    #[test]
    fn hex_charset_sign_pattern() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Lax,
            decimal_separators: &dec,
            grouping_separator: None,
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("D123", "'C'000;'D'000", &props).unwrap();
        assert_eq!(n, "-123");
    }

    #[test]
    fn p_pattern_left() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            decimal_separators: &dec,
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("123", "PP000", &props).unwrap();
        assert_eq!(n, "0.00123");
    }

    #[test]
    #[test]
    #[test]
    fn ppattern_p_on_right() {
        let dec = vec![".".into()];
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Lax,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ignore_case: false,
        };
        let (pre, suf) = negative_affixes("##0PP-", "##0PP+");
        assert_eq!(pre, "");
        assert_eq!(suf, "-");
        let n =
            parse_standard_text_number("123-", "##0PP+;##0PP-", &props).unwrap();
        assert_eq!(n, "-12300");
    }

    #[test]
    fn tnp09_negative_affix() {
        let dec = vec![".".into()];
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let pat = "**######;*/$###### 'is negative!'";
        let n = parse_standard_text_number(
            "******$123456 is negative!",
            pat,
            &props,
        )
        .unwrap();
        assert_eq!(n, "-123456");
    }

    #[test]
    fn pad_escape_x_pattern() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number(
            "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx5,000",
            "*x#,###",
            &props,
        )
        .unwrap();
        assert_eq!(n, "5000");
    }

    #[test]
    fn multi_grouping_pattern() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("123,123,1234", "#,##,###,####", &props).unwrap();
        assert_eq!(n, "1231231234");
    }

    #[test]
    fn scientific_notation_exp_sign() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("1.234E+1", "0.###E+0", &props).unwrap();
        assert_eq!(n, "12.34");
        let n2 = parse_standard_text_number("1.234E-1", "0.###E+0", &props).unwrap();
        assert_eq!(n2, "0.1234");
        let n3 = parse_standard_text_number("12.3E-4", "00.###E0", &props).unwrap();
        assert_eq!(n3, "0.00123");
    }

    fn v_pattern_money() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            decimal_separators: &dec,
            ..TextNumberFormatProps::default()
        };
        let n =
            parse_standard_text_number("999999999", "######0V00;-######0V00", &props).unwrap();
        assert_eq!(n, "9999999.99");
        let n2 = parse_standard_text_number(
            "[999]",
            "[######0V00];(######0V00)",
            &props,
        )
        .unwrap();
        assert_eq!(n2, "9.99");
    }

    #[test]
    fn p_pattern_right() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            decimal_separators: &dec,
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("123", "000PP", &props).unwrap();
        assert_eq!(n, "12300");
    }

    #[test]
    fn hex_charset_sign_pattern_strict() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: None,
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("D123", "'C'000;'D'000", &props).unwrap();
        assert_eq!(n, "-123");
    }

    fn strict_four_zeros() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("1988", "0000", &props).unwrap();
        assert_eq!(n, "1988");
    }

    #[test]
    fn xsd_doubled_apostrophe_in_pattern() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number(
            "                 12 o'clock",
            "* #0 o''clock",
            &props,
        )
        .unwrap();
        assert_eq!(n, "12");
    }

    #[test]
    fn strict_padding_suffix_literals() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number(
            "                 12 o'clock",
            "* #0 o'clock",
            &props,
        )
        .unwrap();
        assert_eq!(n, "12");
        let n2 = parse_standard_text_number(
            "4>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>> right",
            "#*> right",
            &props,
        );
        assert_eq!(n2.as_deref(), Ok("4"), "padding08 got {n2:?}");
        let n3 = parse_standard_text_number(
            "8 Items$$$$$$$$$$$$$$$$$$$$$$$$$$$$$$$$$$$$",
            "# Items*$",
            &props,
        )
        .unwrap();
        assert_eq!(n3, "8");
    }

    #[test]
    fn strict_data_prefix_negative() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number(
            "data:                           4,999",
            "data:* #,###",
            &props,
        )
        .unwrap();
        assert_eq!(n, "4999");
        let n2 = parse_standard_text_number(
            "(data:                           4,999)",
            "data:* #,###;(data:* #,###)",
            &props,
        )
        .unwrap();
        assert_eq!(n2, "-4999");
    }
}
