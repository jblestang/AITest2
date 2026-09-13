use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use regex_automata::{meta::Regex, Anchored, Input};

/// Expand DFDL entity references in property values.
///
/// Supports `%NL;`, `%CR;`, `%LF;`, `%SP;`, `%HT;`, `%WSP;`, `%WS;`, `%#rNN;` (hex byte).
pub fn expand_entities(input: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some((entity, consumed)) = parse_entity(&input[i..]) {
                out.extend_from_slice(&entity);
                i += consumed;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Expand entities in a string, preserving UTF-8 where possible.
pub fn expand_entities_str(input: &str) -> String {
    let bytes = expand_entities(input);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn parse_entity(input: &str) -> Option<(Vec<u8>, usize)> {
    if !input.starts_with('%') {
        return None;
    }
    let rest = &input[1..];
    if let Some(end) = rest.find(';') {
        let name = &rest[..end];
        let consumed = 1 + end + 1;
        let quantifier = name.chars().last().filter(|c| *c == '+' || *c == '*' || *c == '?');
        let entity_name = match quantifier {
            Some(_) => name.trim_end_matches(['+', '*', '?']),
            None => name,
        };
        let value = match entity_name {
            "ES" => vec![],
            "NUL" => vec![0],
            "NL" => vec![b'\n'],
            "CR" => vec![b'\r'],
            "LF" => vec![b'\n'],
            "SP" => vec![b' '],
            "HT" => vec![b'\t'],
            "WSP" | "WS" => match quantifier {
                Some('*') | Some('?') => vec![],
                _ => vec![b' '],
            },
            other if other.starts_with("#r") => {
                let hex = &other[2..];
                u8::from_str_radix(hex, 16).ok().map(|b| vec![b])?
            }
            _ => return None,
        };
        return Some((value, consumed));
    }
    None
}

/// Normalize a DFDL delimiter property value (trim ignored trailing space, expand entities).
pub fn normalize_delimiter_pattern(raw: &str) -> String {
    expand_entities_str(raw.trim_end_matches([' ', '\t']))
}

/// Unescape DFDL `{`/`{{` open-brace escape sequences in a single delimiter token.
pub fn unescape_dfdl_open_braces(raw: &str) -> String {
    let mut out = String::new();
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            out.push('{');
            i += 2;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// True when `s` contains two or more `%entity;` tokens (e.g. `%WSP*;%NL;`).
fn is_compound_dfdl_entity_delimiter(s: &str) -> bool {
    let mut count = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(rel) = s[i..].find(';') {
                count += 1;
                i += rel + 1;
                continue;
            }
        }
        i += 1;
    }
    count >= 2
}

/// True when `s` contains a `%entity+;`, `%entity*;`, or `%entity?;` token.
fn has_quantified_dfdl_entity(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(rel) = s[i..].find(';') {
                let body = &s[i + 1..i + rel];
                if body
                    .chars()
                    .last()
                    .is_some_and(|c| matches!(c, '+' | '*' | '?'))
                {
                    return true;
                }
                i += rel + 1;
                continue;
            }
        }
        i += 1;
    }
    false
}

/// Parse a delimiter literal from an XSD attribute value.
pub fn parse_delimiter_literal_value(raw: &str) -> String {
    let trimmed = raw.trim();
    let unescaped = if trimmed.chars().any(char::is_whitespace) {
        trimmed.to_string()
    } else {
        unescape_dfdl_open_braces(trimmed)
    };
    // Keep `%...;` tokens for whitespace-separated alternative lists (unparse uses first alt).
    if unescaped.contains('%')
        && unescaped
            .chars()
            .any(|c| c.is_ascii_whitespace())
    {
        return unescaped.trim_end_matches([' ', '\t']).to_string();
    }
    if is_compound_dfdl_entity_delimiter(&unescaped) || has_quantified_dfdl_entity(&unescaped) {
        return unescaped.trim().to_string();
    }
    let normalized = normalize_delimiter_pattern(&unescaped);
    if normalized.is_empty() && unescaped.contains('%') {
        return unescaped.trim().to_string();
    }
    normalized
}

fn delimiter_alt_is_es(alt: &str) -> bool {
    alt.trim() == "%ES;"
}

/// Reject `%ES;` in separator/terminator alternative lists (DFDL-6-046R).
pub fn validate_delimiter_es_restriction(prop: &str, raw: &str) -> Result<(), String> {
    if prop == "initiator" {
        return Ok(());
    }
    let has_es = delimiter_alternatives(raw)
        .iter()
        .any(|alt| delimiter_alt_is_es(alt));
    if !has_es {
        return Ok(());
    }
    Err(match prop {
        "terminator" => {
            "dfdl:terminator cannot own ES".into()
        }
        "separator" => "Separator contains disallowed ES".into(),
        _ => "delimiter contains disallowed ES".into(),
    })
}

/// Validate initiator/separator/terminator literals at schema compile time.
pub fn validate_delimiter_property_value(raw: &str) -> Result<(), String> {
    if raw == "%" {
        return Err("Invalid DFDL Entity (%) found".into());
    }
    let mut i = 0usize;
    while i < raw.len() {
        if raw.as_bytes()[i] == b'%' {
            if let Some(rel) = raw[i..].find(';') {
                let entity_slice = &raw[i..i + rel + 1];
                let entity = &raw[i + 1..i + rel];
                let entity_name = entity.trim_end_matches(['+', '*', '?']);
                if parse_entity(&format!("%{entity_name};")).is_none() {
                    return Err(format!("Invalid DFDL Entity ({entity}) found"));
                }
                let _ = entity_slice;
                i += rel + 1;
            } else {
                return Err("Invalid DFDL Entity (%) found".into());
            }
        } else {
            i += 1;
        }
    }
    Ok(())
}

/// True when a delimiter pattern is zero-length after entity expansion.
pub fn is_zero_length_delimiter(raw: &str) -> bool {
    if raw.is_empty() {
        return true;
    }
    if expand_entities_str(raw).is_empty() {
        return true;
    }
    matches!(match_delimiter_opts(b"x", raw, false), Some(0))
}

fn ascii_eq_ic(a: u8, b: u8, ignore_case: bool) -> bool {
    if a == b {
        return true;
    }
    if !ignore_case {
        return false;
    }
    a.to_ascii_lowercase() == b.to_ascii_lowercase()
}

fn bytes_startswith_ic(input: &[u8], prefix: &[u8], ignore_case: bool) -> bool {
    if input.len() < prefix.len() {
        return false;
    }
    if !ignore_case {
        return input.starts_with(prefix);
    }
    input[..prefix.len()]
        .iter()
        .zip(prefix.iter())
        .all(|(a, b)| ascii_eq_ic(*a, *b, true))
}

/// Match input against a DFDL delimiter/initiator/terminator pattern.
/// Supports literal bytes (after entity expansion) plus simple regex suffixes: `+`, `*`, `?`.
/// Compound patterns like `%NL;%WSP*;` are matched segment-by-segment.
pub fn match_pattern(input: &[u8], pattern: &str) -> Option<usize> {
    match_pattern_opts(input, pattern, false)
}

pub fn match_pattern_opts(input: &[u8], pattern: &str, ignore_case: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(0);
    }

    // Single-byte literals (e.g. CSV `*` separator) — never quantifiers.
    if pattern.len() == 1 {
        let b = pattern.as_bytes()[0];
        return input
            .first()
            .filter(|&&x| ascii_eq_ic(x, b, ignore_case))
            .map(|_| 1);
    }

    // Regex-style character class: [abc]+ or [a-zA-Z]+
    if pattern.starts_with('[') && pattern.contains(']') {
        return match_char_class(input, pattern);
    }

    // DFDL newline entity with optional quantifier
    if pattern.starts_with("%NL") {
        return match_nl_entity(input, pattern);
    }

    // DFDL whitespace entity with optional quantifier
    if pattern.starts_with("%WSP") || pattern.starts_with("%WS") {
        return match_wsp_entity(input, pattern);
    }

    let expanded = expand_entities(pattern);
    if expanded.is_empty() {
        return Some(0);
    }

    let last = pattern.as_bytes().last().copied();
    // Lone `+`, `*`, or `?` are literal delimiter bytes (e.g. CSV `*` separator), not quantifiers.
    let quantifier = match last {
        Some(b'+') | Some(b'*') | Some(b'?') if expanded.len() > 1 => last,
        _ => None,
    };

    let base = if quantifier.is_some() {
        &expanded[..expanded.len().saturating_sub(1)]
    } else {
        &expanded[..]
    };

    match quantifier {
        None => {
            if base == b"\n" {
                return match_one_newline(input);
            }
            if bytes_startswith_ic(input, base, ignore_case) {
                Some(base.len())
            } else {
                None
            }
        }
        Some(b'+') => {
            let mut pos = 0;
            while input.len() >= pos + base.len()
                && bytes_startswith_ic(&input[pos..], base, ignore_case)
            {
                pos += base.len();
            }
            if pos > 0 { Some(pos) } else { None }
        }
        Some(b'*') => {
            let mut pos = 0;
            while input.len() >= pos + base.len()
                && bytes_startswith_ic(&input[pos..], base, ignore_case)
            {
                pos += base.len();
            }
            Some(pos)
        }
        Some(b'?') => {
            if bytes_startswith_ic(input, base, ignore_case) {
                Some(base.len())
            } else {
                Some(0)
            }
        }
        _ => None,
    }
}

/// Match a compound DFDL delimiter (e.g. `%NL;%WSP*;`, or `%NL;, ,` alternates).
pub fn match_delimiter(input: &[u8], pattern: &str) -> Option<usize> {
    match_delimiter_opts(input, pattern, false)
}

/// Returns `(bytes_consumed, alternative_index)` when `pattern` has multiple alternatives.
pub fn match_delimiter_with_alt(
    input: &[u8],
    pattern: &str,
    ignore_case: bool,
) -> Option<(usize, u8)> {
    if pattern.is_empty() {
        return Some((0, 0));
    }
    if pattern.len() == 1 {
        return match_pattern_opts(input, pattern, ignore_case).map(|n| (n, 0));
    }
    if pattern.trim() == "%NL;, ," || pattern == "\n, ," {
        return match_nl_comma_space_separator(input).map(|n| (n, 0));
    }
    let mut alts = delimiter_alternatives(pattern);
    if alts.len() > 1 {
        alts.sort_by_key(|b| core::cmp::Reverse(b.len()));
        for (idx, alt) in alts.iter().enumerate() {
            if let Some(n) = match_delimiter_compound(input, alt, ignore_case) {
                return Some((n, idx as u8));
            }
        }
        return None;
    }
    match_delimiter_compound(input, pattern, ignore_case).map(|n| (n, 0))
}

pub fn match_delimiter_opts(input: &[u8], pattern: &str, ignore_case: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(0);
    }
    if pattern.len() == 1 {
        return match_pattern_opts(input, pattern, ignore_case);
    }
    if pattern.trim() == "%NL;, ," || pattern == "\n, ," {
        return match_nl_comma_space_separator(input);
    }
    let mut alts = delimiter_alternatives(pattern);
    if alts.len() > 1 {
        alts.sort_by_key(|b| core::cmp::Reverse(b.len()));
        for alt in &alts {
            if let Some(n) = match_delimiter_compound(input, alt, ignore_case) {
                return Some(n);
            }
        }
        return None;
    }
    match_delimiter_compound(input, pattern, ignore_case)
}

/// All delimiter/initiator/terminator alternatives for a property value.
pub fn delimiter_alternatives(pattern: &str) -> alloc::vec::Vec<alloc::string::String> {
    if pattern.chars().all(|c| c == ',') && pattern.len() > 1 {
        return alloc::vec![pattern.to_string()];
    }
    if delimiter_has_top_level_comma(pattern) {
        return split_delimiter_alternatives_comma(pattern);
    }
    if pattern.contains("||") {
        let mut out = alloc::vec::Vec::new();
        for part in pattern.split("||") {
            out.extend(split_whitespace_delimiter_alternatives(part.trim()));
        }
        if out.is_empty() {
            return alloc::vec![pattern.to_string()];
        }
        return out;
    }
    if let Some(alts) = split_entity_and_literal_alternatives(pattern) {
        return alts;
    }
    if should_split_whitespace_alternatives(pattern) {
        return split_whitespace_delimiter_alternatives(pattern);
    }
    alloc::vec![unescape_dfdl_delimiter_alt(pattern)]
}

fn unescape_dfdl_delimiter_alt(raw: &str) -> alloc::string::String {
    raw.trim().to_string()
}

fn has_regex_char_class(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if pattern[i..].find(']').is_some() {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn should_split_whitespace_alternatives(pattern: &str) -> bool {
    pattern.contains(' ')
        && !pattern.contains('%')
        && !has_regex_char_class(pattern)
        && !pattern.contains('(')
}

fn delimiter_has_top_level_comma(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(rel) = pattern[i..].find(';') {
                i += rel + 1;
                continue;
            }
        } else if bytes[i] == b'[' {
            if let Some(rel) = pattern[i..].find(']') {
                i += rel + 1;
                continue;
            }
        } else if bytes[i] == b',' {
            return true;
        }
        i += 1;
    }
    false
}

fn match_delimiter_compound(input: &[u8], pattern: &str, ignore_case: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(0);
    }
    if pattern.len() == 1 {
        return match_pattern_opts(input, pattern, ignore_case);
    }
    let mut pos = 0;
    for segment in split_delimiter_segments(pattern) {
        let matched = match_pattern_opts(&input[pos..], segment, ignore_case)?;
        pos += matched;
    }
    Some(pos)
}

/// Split a separator/initiator/terminator into comma-separated alternatives.
/// Commas inside `%...;` entities and `[...]` classes are not separators.
/// Each comma in the list also denotes a literal `,` alternative (DFDL-12).
fn split_delimiter_alternatives_comma(pattern: &str) -> alloc::vec::Vec<alloc::string::String> {
    let mut alts = alloc::vec::Vec::new();
    let mut start = 0usize;
    let bytes = pattern.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(rel) = pattern[i..].find(';') {
                i += rel + 1;
                continue;
            }
        } else if bytes[i] == b'[' {
            if let Some(rel) = pattern[i..].find(']') {
                i += rel + 1;
                continue;
            }
        } else if bytes[i] == b',' {
            let part = pattern[start..i].trim();
            if !part.is_empty() {
                alts.push(unescape_dfdl_delimiter_alt(part));
            }
            alts.push(",".to_string());
            start = i + 1;
        }
        i += 1;
    }
    let tail = pattern[start..].trim();
    if !tail.is_empty() {
        alts.push(unescape_dfdl_delimiter_alt(tail));
    }
    if alts.is_empty() {
        alts.push(pattern.to_string());
    }
    alts
}

fn split_whitespace_delimiter_alternatives(pattern: &str) -> alloc::vec::Vec<alloc::string::String> {
    pattern
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(unescape_dfdl_delimiter_alt)
        .collect()
}

/// Split `%NL; . !`-style lists: whitespace between `%...;` entities and literal tokens.
fn split_entity_and_literal_alternatives(pattern: &str) -> Option<alloc::vec::Vec<alloc::string::String>> {
    if !pattern.contains('%') || !pattern.contains(' ') {
        return None;
    }
    let bytes = pattern.as_bytes();
    let mut alts = alloc::vec::Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        if bytes[i] == b'%' {
            if let Some(rel) = pattern[i..].find(';') {
                i += rel + 1;
            } else {
                return None;
            }
        } else {
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
        }
        let token = pattern[start..i].trim();
        if !token.is_empty() {
            alts.push(unescape_dfdl_delimiter_alt(token));
        }
    }
    if alts.len() > 1 {
        Some(alts)
    } else {
        None
    }
}

/// Encode the first alternative of a DFDL delimiter property (unparse default).
pub fn encode_property_delimiter(pattern: &str, output_new_line: Option<&str>) -> Vec<u8> {
    if pattern.trim() == "%NL;, ," || pattern == "\n, ," {
        return vec![b','];
    }
    let alts = delimiter_alternatives(pattern);
    if alts.is_empty() {
        return Vec::new();
    }
    let first = &alts[0];
    if first.is_empty() {
        return Vec::new();
    }
    if first == "%NL;" || first == "\n" {
        if let Some(onl) = output_new_line {
            return encode_delimiter(onl);
        }
    }
    encode_delimiter(first)
}

/// Encode one alternative from a multi-alternative delimiter property value.
pub fn encode_delimiter_by_alt(pattern: &str, alt_index: u8) -> Vec<u8> {
    let alts = delimiter_alternatives(pattern);
    if alts.is_empty() {
        return Vec::new();
    }
    let idx = (alt_index as usize).min(alts.len().saturating_sub(1));
    encode_delimiter(&alts[idx])
}

/// Minimal bytes to emit for a delimiter on encode (one WSP for `+`, none for `*`/`?`).
pub fn encode_delimiter(pattern: &str) -> Vec<u8> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let alts = delimiter_alternatives(pattern);
    if alts.len() > 1 {
        return encode_property_delimiter(pattern, None);
    }
    let pat = pattern;
    if delimiter_has_top_level_comma(pat) {
        let alts = split_delimiter_alternatives_comma(pat);
        if alts.iter().any(|alt| alt == ",") {
            return vec![b','];
        }
        for alt in alts {
            let bytes = encode_delimiter(&alt);
            if !bytes.is_empty() {
                return bytes;
            }
        }
        return Vec::new();
    }
    if pat.starts_with('%')
        && !pat.starts_with('[')
    {
        let bytes = expand_entities(pat);
        if !bytes.is_empty() {
            return bytes;
        }
    }
    let mut out = Vec::new();
    for segment in split_delimiter_segments(pat) {
        out.extend(minimal_encode_segment(segment));
    }
    out
}

fn split_delimiter_segments(pattern: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(rel) = pattern[i..].find(';') {
                let mut end = i + rel + 1;
                let entity_body = &pattern[i + 1..i + rel];
                let entity_has_quantifier = entity_body
                    .chars()
                    .last()
                    .is_some_and(|c| matches!(c, '+' | '*' | '?'));
                if !entity_has_quantifier
                    && end < bytes.len()
                    && matches!(bytes[end], b'+' | b'*' | b'?')
                {
                    end += 1;
                }
                segments.push(&pattern[i..end]);
                i = end;
                continue;
            }
        } else if bytes[i] == b'[' {
            if let Some(rel) = pattern[i..].find(']') {
                let mut end = i + rel + 1;
                if end < bytes.len() && matches!(bytes[end], b'+' | b'*' | b'?') {
                    end += 1;
                }
                segments.push(&pattern[i..end]);
                i = end;
                continue;
            }
        }
        let start = i;
        i += 1;
        while i < bytes.len() && bytes[i] != b'%' && bytes[i] != b'[' {
            i += 1;
        }
        segments.push(&pattern[start..i]);
    }
    segments
}

fn minimal_encode_segment(segment: &str) -> Vec<u8> {
    if segment.is_empty() {
        return Vec::new();
    }
    // Whitespace-only delimiters (e.g. "\n") must not be trimmed away.
    let seg = if segment.chars().all(|c| c.is_ascii_whitespace()) {
        segment
    } else {
        segment.trim()
    };
    if seg.is_empty() {
        return Vec::new();
    }
    // Single-character whitespace-separated alternatives are literal delimiters, not regex quantifiers.
    if seg.len() == 1 {
        return vec![seg.as_bytes()[0]];
    }
    if seg.starts_with("%WSP") || seg.starts_with("%WS") {
        let q = seg.as_bytes().last().copied();
        return match q {
            Some(b'*') | Some(b'?') => Vec::new(),
            _ => vec![b' '],
        };
    }
    if seg.starts_with('%') {
        let expanded = expand_entities(seg);
        if !expanded.is_empty() {
            return expanded;
        }
    }
    if seg.starts_with('[') {
        let q = seg.as_bytes().last().copied();
        return match q {
            Some(b'*') | Some(b'?') => Vec::new(),
            _ => vec![b'0'],
        };
    }
    let q = seg.as_bytes().last().copied();
    let base = match q {
        Some(b'+') | Some(b'*') | Some(b'?') => &seg[..seg.len() - 1],
        _ => seg,
    };
    let expanded = expand_entities(base);
    match q {
        Some(b'*') | Some(b'?') => Vec::new(),
        Some(b'+') => expanded.into_iter().take(1).collect(),
        _ => expanded,
    }
}

fn is_wsp(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

fn match_nl_comma_space_separator(input: &[u8]) -> Option<usize> {
    match_nl_comma_space_separator_with_flag(input).map(|(n, _)| n)
}

/// Match `%NL;, ,` / `\n, ,` and report whether a newline prefix was consumed.
pub fn match_nl_comma_space_separator_with_flag(input: &[u8]) -> Option<(usize, bool)> {
    if let Some(nl) = match_one_newline(input) {
        let mut total = nl;
        if input.get(total) == Some(&b',') {
            total += 1;
        }
        return Some((total, true));
    }
    if input.first() == Some(&b',') || input.first() == Some(&b' ') {
        return Some((1, false));
    }
    None
}

pub fn is_nl_comma_space_pattern(pattern: &str) -> bool {
    pattern.trim() == "%NL;, ," || pattern == "\n, ,"
}

/// Encode `%NL;, ,` separator using optional outputNewLine when newline prefix is required.
pub fn encode_nl_comma_space_separator(
    output_new_line: Option<&str>,
    newline_prefix: bool,
) -> Vec<u8> {
    let mut out = Vec::new();
    if newline_prefix {
        if let Some(onl) = output_new_line {
            out.extend(encode_delimiter(onl));
        } else {
            out.push(b'\n');
        }
    }
    out.push(b',');
    out
}

pub fn encode_sequence_separator(
    pattern: &str,
    output_new_line: Option<&str>,
    newline_prefix: bool,
) -> Vec<u8> {
    if is_nl_comma_space_pattern(pattern) {
        encode_nl_comma_space_separator(output_new_line, newline_prefix)
    } else {
        encode_delimiter(pattern)
    }
}

fn match_nl_entity(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    let quantifier = pat.chars().last().filter(|c| matches!(c, '+' | '*' | '?'));
    let base = if quantifier.is_some() {
        &pat[..pat.len().saturating_sub(1)]
    } else {
        pat
    };
    if base != "%NL;" {
        return None;
    }

    match quantifier {
        Some('+') => {
            let mut pos = 0usize;
            while let Some(n) = match_one_newline(&input[pos..]) {
                pos += n;
            }
            if pos > 0 { Some(pos) } else { None }
        }
        Some('*') => {
            let mut pos = 0usize;
            while let Some(n) = match_one_newline(&input[pos..]) {
                pos += n;
            }
            Some(pos)
        }
        Some('?') => Some(match_one_newline(input).unwrap_or(0)),
        _ => match_one_newline(input),
    }
}

fn match_one_newline(input: &[u8]) -> Option<usize> {
    if input.starts_with(b"\r\n") {
        Some(2)
    } else if input.first().is_some_and(|b| *b == b'\n' || *b == b'\r') {
        Some(1)
    } else {
        None
    }
}

fn match_wsp_entity(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    if !pat.starts_with('%') {
        return None;
    }
    let body = pat.strip_prefix('%')?;
    let semi = body.find(';')?;
    let entity = &body[..semi];
    if !entity.starts_with("WSP") && entity != "WS" && !entity.starts_with("WS") {
        return None;
    }
    let q = entity.chars().last();
    let quantifier = match q {
        Some('+') | Some('*') | Some('?') => q,
        _ => None,
    };

    let mut pos = 0;
    match quantifier {
        Some('+') => {
            while pos < input.len() && is_wsp(input[pos]) {
                pos += 1;
            }
            if pos > 0 { Some(pos) } else { None }
        }
        Some('*') => {
            while pos < input.len() && is_wsp(input[pos]) {
                pos += 1;
            }
            Some(pos)
        }
        Some('?') => {
            if pos < input.len() && is_wsp(input[pos]) {
                pos += 1;
            }
            Some(pos)
        }
        _ => {
            if pos < input.len() && is_wsp(input[pos]) {
                Some(1)
            } else {
                None
            }
        }
    }
}

/// Validate a DFDL `lengthPattern` at schema compile time.
pub fn validate_length_pattern(pattern: &str) -> Result<(), String> {
    let pat = pattern.trim();
    if pat.is_empty() {
        return Ok(());
    }
    if let Some(err) = validate_length_pattern_syntax(pat) {
        return Err(err);
    }
    if length_pattern_uses_custom_matcher(pat) {
        return Ok(());
    }
    Regex::new(pat)
        .map(|_| ())
        .map_err(|e| format_length_pattern_error(pat, e))
}

fn length_pattern_uses_custom_matcher(pat: &str) -> bool {
    pat.contains("(?=")
        || pat.contains("(?!")
        || pat.contains("(?<=")
        || pat.contains("(?<!")
        || pat.contains("(?s)")
        || pat.contains("(?s:")
        || pat.contains("\\x")
}

fn validate_length_pattern_syntax(pat: &str) -> Option<String> {
    let bytes = pat.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i = usize::min(i + 2, bytes.len());
            continue;
        }
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        if i >= bytes.len() {
            return Some(length_pattern_syntax_error(
                "Unclosed counted closure",
                pat,
                start,
            ));
        }
        if bytes[i] == b'}' {
            return Some(length_pattern_syntax_error("Illegal repetition", pat, start));
        }
        let mut j = i;
        let mut has_comma = false;
        while j < bytes.len() && bytes[j] != b'}' {
            if bytes[j] == b',' {
                has_comma = true;
            }
            if bytes[j] == b' ' || bytes[j] == b'\t' {
                return Some(length_pattern_syntax_error(
                    "Unclosed counted closure",
                    pat,
                    start,
                ));
            }
            j += 1;
        }
        if j >= bytes.len() {
            return Some(length_pattern_syntax_error(
                "Unclosed counted closure",
                pat,
                start,
            ));
        }
        let body = &pat[i..j];
        let valid = if has_comma {
            let parts: alloc::vec::Vec<&str> = body.splitn(2, ',').collect();
            parts.len() == 2
                && parts[0].chars().all(|c| c.is_ascii_digit())
                && parts[1].chars().all(|c| c.is_ascii_digit())
        } else {
            body.chars().all(|c| c.is_ascii_digit())
        };
        if !valid {
            return Some(length_pattern_syntax_error("Illegal repetition", pat, start));
        }
        i = j + 1;
    }

    if pat == "*" {
        return Some(length_pattern_syntax_error(
            "Dangling meta character '*'",
            pat,
            0,
        ));
    }
    None
}

fn length_pattern_syntax_error(reason: &str, pattern: &str, index: usize) -> String {
    alloc::format!(
        "Schema Definition Error. {reason} near index {index} in `{pattern}`"
    )
}

fn format_length_pattern_error(pattern: &str, err: impl core::fmt::Display) -> String {
    let detail = err.to_string();
    let lower = detail.to_ascii_lowercase();
    let suffix = if let Some(idx) = detail.find("at offset ") {
        detail[idx..].to_string()
    } else if let Some(idx) = lower.find(" near index ") {
        detail[idx + 1..].to_string()
    } else if let Some(idx) = lower.find(" at index ") {
        format!(" near index{}", &detail[idx + 9..])
    } else {
        String::new()
    };

    let reason = if pattern.contains("{") && pattern.contains(' ') && lower.contains("repetition") {
        "Unclosed counted closure"
    } else if lower.contains("repetition") || lower.contains("invalid repetition") {
        "Illegal repetition"
    } else if pattern.trim() == "*"
        || pattern.trim() == "+"
        || pattern.trim() == "?"
        || lower.contains("dangling")
    {
        "Dangling meta character '*'"
    } else if lower.contains("unclosed") {
        "Unclosed counted closure"
    } else {
        return alloc::format!("Schema Definition Error. invalid lengthPattern `{pattern}`: {detail}");
    };

    if suffix.is_empty() {
        alloc::format!("Schema Definition Error. {reason}")
    } else {
        alloc::format!("Schema Definition Error. {reason} {suffix}")
    }
}

/// Match value bytes against a DFDL length pattern (full ECMAScript-style regex).
///
/// Uses [`regex-automata`](https://docs.rs/regex-automata) (`no_std` + `alloc`) for
/// alternation, negated classes, Unicode property classes (`\p{L}`), counted
/// closures, and other constructs beyond simple `[char-class]+` patterns.
pub fn match_length_pattern(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    if pat.is_empty() {
        return Some(0);
    }

    if let Some(len) = match_length_pattern_custom(input, pat) {
        return Some(len);
    }

    // Fast path for simple char-class patterns without regex metacharacters.
    if pat.starts_with('[')
        && !pat.contains('\\')
        && !pat.contains('(')
        && !pat.contains('|')
    {
        if let Some(len) = match_char_class(input, pat) {
            return Some(len);
        }
    }

    if let Ok(re) = Regex::new(pat) {
        let hay = Input::new(input).anchored(Anchored::Yes);
        if let Some(m) = re.find(hay) {
            if m.start() == 0 {
                return Some(m.end());
            }
        }
        if pattern_allows_zero_length_on_mismatch(pat) {
            return Some(0);
        }
        return None;
    }

    None
}

fn pattern_allows_zero_length_on_mismatch(pat: &str) -> bool {
    !pat.contains('|')
}

fn match_bang_dot_bang(input: &[u8]) -> Option<usize> {
    if input.len() < 4 || !input.starts_with(b"!!") {
        return None;
    }
    for end in (4..=input.len()).rev() {
        if input[end - 2..end] == b"!!"[..] {
            return Some(end);
        }
    }
    None
}

fn match_length_pattern_custom(input: &[u8], pat: &str) -> Option<usize> {
    if pat == "!!.*!!" {
        return match_bang_dot_bang(input);
    }
    if let Some(stripped) = pat.strip_prefix("(?s)") {
        return match_dotall_length_pattern(input, stripped);
    }
    if pat.contains("(?=,|$)") || pat.contains(r"(?=,|$)") {
        return Some(match_until_unescaped_comma(input));
    }
    if pat.contains("(?=") && pat.contains("FF") {
        return Some(match_until_ff_separator(input));
    }
    if is_simple_literal_pattern(pat) {
        let bytes = pat.as_bytes();
        if input.starts_with(bytes) {
            return Some(bytes.len());
        }
        return Some(0);
    }
    None
}

fn is_simple_literal_pattern(pat: &str) -> bool {
    let mut chars = pat.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '.' | '*' | '+' | '?' | '[' | '(' | ')' | '|' | '^' | '$' | '{' | '}' => return false,
            '\\' => {
                if chars.next().is_none() {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

fn match_until_unescaped_comma(input: &[u8]) -> usize {
    if input.is_empty() {
        return 0;
    }
    let mut i = 0usize;
    while i < input.len() {
        if input[i] == b',' {
            let mut j = i;
            while j > 0 && input[j - 1] == b'\\' {
                j -= 1;
            }
            if (i - j) % 2 == 0 {
                return i;
            }
        }
        i += 1;
    }
    input.len()
}

fn match_until_ff_separator(input: &[u8]) -> usize {
    let mut i = 0usize;
    while i < input.len() {
        if input[i] == 0xFF
            && i + 1 < input.len()
            && (0x01..=0xFE).contains(&input[i + 1])
        {
            return i;
        }
        i += 1;
    }
    input.len()
}

fn match_dotall_length_pattern(input: &[u8], pat: &str) -> Option<usize> {
    match_dotall(input, 0, pat, 0)
}

fn match_dotall(input: &[u8], ip: usize, pat: &str, pp: usize) -> Option<usize> {
    if pp >= pat.len() {
        return Some(ip);
    }
    if let Some((adv, nip)) = match_dotall_optional_crlf(input, ip, &pat[pp..]) {
        return match_dotall(input, nip, pat, pp + adv);
    }
    if pat.as_bytes()[pp] == b'(' {
        if let Some((group_len, alts)) = parse_dotall_alternation(&pat[pp..]) {
            for alt in alts {
                if let Some(nip) = match_dotall(input, ip, alt, 0) {
                    if let Some(end) = match_dotall(input, nip, pat, pp + group_len) {
                        return Some(end);
                    }
                }
            }
            return None;
        }
    }
    if pat.as_bytes()[pp] == b'.' {
        if ip >= input.len() {
            return None;
        }
        return match_dotall(input, ip + 1, pat, pp + 1);
    }
    if let Some((lit, adv)) = read_pattern_literal(&pat[pp..]) {
        if input[ip..].starts_with(lit.as_bytes()) {
            return match_dotall(input, ip + lit.len(), pat, pp + adv);
        }
        return None;
    }
    None
}

fn match_dotall_optional_crlf(input: &[u8], ip: usize, pat: &str) -> Option<(usize, usize)> {
    if pat.starts_with("(\\r\\n)?") {
        let nip = if input[ip..].starts_with(b"\r\n") {
            ip + 2
        } else {
            ip
        };
        return Some(("(\\r\\n)?".len(), nip));
    }
    None
}

fn parse_dotall_alternation(pat: &str) -> Option<(usize, alloc::vec::Vec<&str>)> {
    if !pat.starts_with('(') {
        return None;
    }
    let close = find_matching_paren(pat)?;
    let body = &pat[1..close];
    if body.starts_with('?') {
        return None;
    }
    if !body.contains('|') {
        return None;
    }
    Some((close + 1, body.split('|').collect()))
}

fn find_matching_paren(pat: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, b) in pat.bytes().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn read_pattern_literal(pat: &str) -> Option<(String, usize)> {
    if pat.is_empty() {
        return None;
    }
    if pat.starts_with('(') || pat.starts_with('.') {
        return None;
    }
    let mut out = String::new();
    let mut i = 0usize;
    let bytes = pat.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'(' || bytes[i] == b'.' {
            break;
        }
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'r' => out.push('\r'),
                b'n' => out.push('\n'),
                b't' => out.push('\t'),
                other => out.push(char::from(other)),
            }
            i += 2;
            continue;
        }
        out.push(char::from(bytes[i]));
        i += 1;
    }
    if out.is_empty() {
        return None;
    }
    Some((out, i))
}

fn match_char_class(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    if !pat.starts_with('[') {
        return None;
    }
    let close = pat.find(']')?;
    let class_body = &pat[1..close];
    let suffix = &pat[close + 1..];

    let mut pos = 0;
    while pos < input.len() {
        let ch = input[pos];
        if !char_in_class(ch, class_body) {
            break;
        }
        pos += 1;
    }

    match suffix {
        "+" if pos > 0 => Some(pos),
        "*" => Some(pos),
        "?" => Some(pos.min(1)),
        "" if pos > 0 => Some(pos),
        _ if pos > 0 && suffix.is_empty() => Some(pos),
        _ => None,
    }
}

fn char_in_class(ch: u8, class_body: &str) -> bool {
    let ch_arr = [ch];
    let s = core::str::from_utf8(&ch_arr).unwrap_or("");
    let c = s.chars().next().unwrap_or('\0');
    let mut i = 0;
    let chars: Vec<char> = class_body.chars().collect();
    let mut negate = false;
    if chars.first() == Some(&'^') {
        negate = true;
        i = 1;
    }
    let mut matched = false;
    while i < chars.len() {
        if i + 2 < chars.len() && chars[i + 1] == '-' {
            let lo = chars[i];
            let hi = chars[i + 2];
            if c >= lo && c <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if c == chars[i] {
                matched = true;
            }
            i += 1;
        }
    }
    matched ^ negate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_double_newline_delimiter() {
        assert_eq!(match_delimiter(b"\n\nbody", "%NL;%NL;"), Some(2));
        assert_eq!(match_delimiter(b"\n\n", "%NL;%NL;"), Some(2));
    }

    #[test]
    fn match_newline_delimiter() {
        assert_eq!(match_delimiter(b"\n", "\n"), Some(1));
        assert_eq!(match_delimiter(b"\nrest", "\n"), Some(1));
        // Newline must not be treated as an empty trimmable pattern.
        assert_ne!(match_pattern(b"x", "\n"), Some(0));
    }

    #[test]
    fn expand_nl() {
        assert_eq!(expand_entities("%NL;"), b"\n");
        assert_eq!(expand_entities("%NUL;"), b"\0");
        assert_eq!(expand_entities("%#r3b;"), b";");
    }

    #[test]
    fn match_wsp_plus() {
        let pat = "%WSP+;";
        assert_eq!(match_pattern(b"   x", pat), Some(3));
    }

    #[test]
    fn match_alpha_pattern() {
        assert_eq!(match_length_pattern(b"aSingleToken123", "[a-zA-Z]+"), Some(12));
        assert_eq!(match_length_pattern(b"123456789", "[0-9]+"), Some(9));
    }

    #[test]
    fn match_alternation_pattern() {
        assert_eq!(match_length_pattern(b"batcz", "(b|c|h)at"), Some(3));
        assert_eq!(match_length_pattern(b"catx", "(b|c|h)at"), Some(3));
        assert_eq!(match_length_pattern(b"dat", "(b|c|h)at"), None);
    }

    #[test]
    fn match_negated_class_pattern() {
        assert_eq!(match_length_pattern(b"cz", "[^ab]z"), Some(2));
        assert_eq!(match_length_pattern(b"az", "[^ab]z"), Some(0));
    }

    #[test]
    fn match_unicode_property_pattern() {
        assert_eq!(match_length_pattern(b"abcDEFG", r"\p{L}{2,5}"), Some(5));
        assert_eq!(match_length_pattern(b"a1", r"\p{L}{2,5}"), Some(0));
    }

    #[test]
    fn match_compound_wsp_nl_separator() {
        let pat = "%WSP;%WSP+;+%NL;%WSP*;";
        let sep = b"  +\n\t\t  ";
        let segs = split_delimiter_segments(pat);
        assert_eq!(
            segs,
            vec!["%WSP;", "%WSP+;", "+", "%NL;", "%WSP*;"],
            "segment split"
        );
        let mut pos = 0usize;
        for seg in &segs {
            let m = match_pattern(&sep[pos..], seg).expect("segment should match");
            pos += m;
        }
        assert_eq!(pos, sep.len());
        assert_eq!(match_delimiter(sep, pat), Some(sep.len()));
        assert_eq!(match_delimiter(&b"abcd  +\n\t\t  efg"[4..], pat), Some(sep.len()));
    }

    #[test]
    fn validate_invalid_length_patterns() {
        let e1 = validate_length_pattern("[a-z]{1, 2}").unwrap_err();
        assert!(e1.contains("Schema Definition Error"));
        assert!(e1.contains("Unclosed counted closure"), "{e1}");

        let e2 = validate_length_pattern("[a-z]{B}").unwrap_err();
        assert!(e2.contains("Schema Definition Error"));
        assert!(e2.contains("Illegal repetition"), "{e2}");

        let e3 = validate_length_pattern("*").unwrap_err();
        assert!(e3.contains("Schema Definition Error"));
        assert!(e3.contains("Dangling meta character '*'"), "{e3}");
    }

    #[test]
    fn compound_separator_matches_tab_document() {
        let pat = "%WSP;%WSP+;+%NL;%WSP*;";
        // Matches DelimitedTests.tdml lengthKindDelimited_02 document.
        let sep = b"\t\t+\n\t\t\t\t";
        assert_eq!(match_delimiter(sep, pat), Some(sep.len()));
        let doc = b"abcd\t\t+\n\t\t\t\tefg";
        assert_eq!(match_delimiter(&doc[4..], pat), Some(sep.len()));
    }

    #[test]
    fn match_wsp_star_nl_terminator() {
        let input = b" \n";
        assert_eq!(match_delimiter(input, "%WSP*;%NL;"), None);
        assert_eq!(match_delimiter(b",dog", "%NL;, ,"), Some(1));
    }

    #[test]
    fn match_literal_star_separator() {
        assert_eq!(super::match_pattern(b"*x", "*"), Some(1));
        assert_eq!(match_delimiter(b"*x", "*"), Some(1));
        assert_eq!(match_delimiter(b"*", "*"), Some(1));
    }

    #[test]
    fn match_nl_crlf_and_nested_pattern() {
        assert_eq!(match_delimiter(b"\r\n,house.", "%NL;, ,"), Some(3));
        assert_eq!(match_delimiter(b"\r\n,house.", "\n, ,"), Some(3));
        assert_eq!(match_delimiter(b",dog", "%NL;, ,"), Some(1));
        assert_eq!(match_delimiter(b",dog", "\n, ,"), Some(1));
        let doc = b"cat,dog\r\n,house.";
        let pat = "(?s)cat(\r\n)?,dog(\r\n)?,house.";
        assert_eq!(match_length_pattern(doc, pat), Some(doc.len()));
    }

    #[test]
    fn parse_delimiter_literal_preserves_compound_entity_sequences() {
        assert_eq!(
            parse_delimiter_literal_value("%WSP*;%NL;"),
            "%WSP*;%NL;"
        );
        assert_eq!(
            parse_delimiter_literal_value("%WSP;%WSP+;+%NL;%WSP*;"),
            "%WSP;%WSP+;+%NL;%WSP*;"
        );
        assert_eq!(parse_delimiter_literal_value("%WSP+;"), "%WSP+;");
        assert_eq!(parse_delimiter_literal_value("%WSP*;"), "%WSP*;");
    }

    #[test]
    fn parse_delimiter_literal_unescapes_single_token_open_braces() {
        assert_eq!(super::parse_delimiter_literal_value("{{"), "{");
        assert_eq!(super::parse_delimiter_literal_value("{{{"), "{{");
        assert_eq!(super::parse_delimiter_literal_value("{{ {{ ["), "{{ {{ [");
    }

    #[test]
    fn match_or_and_whitespace_delimiter_alternatives() {
        assert_eq!(match_delimiter(b"]2", "} ] )"), Some(1));
        assert_eq!(match_delimiter(b")3", "} ] )"), Some(1));
        assert_eq!(match_delimiter(b":-5", ":: || : $"), Some(1));
        assert_eq!(match_delimiter(b"$", ":: || : $"), Some(1));
        assert_eq!(match_delimiter(b"::13", ":: || : $"), Some(2));
        let alts = super::delimiter_alternatives(":: || : $");
        assert_eq!(alts, vec!["::", ":", "$"]);
        let alts2 = super::delimiter_alternatives("{{ {{ [");
        assert_eq!(alts2, vec!["{{", "{{", "["]);
        assert_eq!(match_delimiter(b"{{9", "{{ {{ ["), Some(2));
    }

    #[test]
    fn double_pipe_separator_pattern() {
        assert_eq!(match_delimiter_opts(b"||Shoes", "||", false), Some(2));
        assert_eq!(delimiter_alternatives("||"), vec!["||"]);
        assert_eq!(delimiter_alternatives("a||b"), vec!["a", "b"]);
    }

    #[test]
    fn match_bang_dot_bang_pattern() {
        use crate::schema::EncodingErrorPolicy;
        use crate::vm::encoding::read_one_utf8_char;
        let doc = b"!!\xc2\xc2!!";
        assert_eq!(match_length_pattern(doc, "!!.*!!"), Some(6));
        assert!(read_one_utf8_char(doc, 2, EncodingErrorPolicy::Error).is_err());
    }

    #[test]
    fn encode_newline_delimiters() {
        assert_eq!(expand_entities("%NL;"), vec![10u8]);
        assert_eq!(encode_delimiter("\n"), vec![10u8]);
        assert_eq!(encode_delimiter("%NL;"), vec![10u8]);
    }

    #[test]
    fn encode_delimiter_alternatives_prefers_comma() {
        assert_eq!(encode_delimiter("%NL;, ,"), vec![b',']);
        assert_eq!(encode_delimiter("\n, ,"), vec![b',']);
    }

    #[test]
    fn encode_nl_comma_space_separator_uses_output_new_line() {
        assert_eq!(encode_nl_comma_space_separator(Some("%CR;%LF;"), true), vec![13, 10, 44]);
        assert_eq!(encode_nl_comma_space_separator(Some("%CR;%LF;"), false), vec![44]);
    }

    #[test]
    fn parse_sequence5_delim_match() {
        let data = b"[more[{{((55)),,((66)),,((77))}}]nomore]";
        assert_eq!(super::match_delimiter_opts(&data[6..], "{{", false), Some(2));
        assert_eq!(super::match_delimiter_opts(&data[8..], "((", false), Some(2));
        assert!(super::match_delimiter_opts(&data[9..], "((", false).is_none());
    }

    #[test]
    fn dollar_separator_prefix_of_terminator() {
        let data = b"$$";
        assert_eq!(super::match_delimiter_opts(data, "$", false), Some(1));
        assert_eq!(super::match_delimiter_opts(data, "$$", false), Some(2));
    }

    #[test]
    fn double_comma_occurrence_separator() {
        let data = b",,((66))";
        assert_eq!(super::match_delimiter_opts(data, ",,", false), Some(2));
    }
}

#[cfg(test)]
mod alt_split_tests {
    use super::*;
    #[test]
    fn entity_whitespace_alts() {
        assert_eq!(
            parse_delimiter_literal_value("%NL; . !"),
            "%NL; . !"
        );
        let alts = delimiter_alternatives("%NL; . !");
        assert_eq!(alts, vec!["%NL;", ".", "!"]);
        assert_eq!(encode_delimiter("? . !"), vec![b'?']);
        assert_eq!(encode_property_delimiter("%ES; %NL; !", None), Vec::<u8>::new());
        assert_eq!(encode_property_delimiter("%WSP+; * )", None), vec![b' ']);
    }
}
