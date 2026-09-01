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
        let value = match name {
            "NUL" => vec![0],
            "NL" => vec![b'\n'],
            "CR" => vec![b'\r'],
            "LF" => vec![b'\n'],
            "SP" => vec![b' '],
            "HT" => vec![b'\t'],
            "WSP" | "WS" => vec![b' '], // canonical whitespace for entity expansion
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

/// Match input against a DFDL delimiter/initiator/terminator pattern.
/// Supports literal bytes (after entity expansion) plus simple regex suffixes: `+`, `*`, `?`.
/// Compound patterns like `%NL;%WSP*;` are matched segment-by-segment.
pub fn match_pattern(input: &[u8], pattern: &str) -> Option<usize> {
    if pattern.is_empty() {
        return Some(0);
    }

    // Single-byte literals (e.g. CSV `*` separator) — never quantifiers.
    if pattern.len() == 1 {
        let b = pattern.as_bytes()[0];
        return if input.first() == Some(&b) { Some(1) } else { None };
    }

    // Regex-style character class: [abc]+ or [a-zA-Z]+
    if pattern.starts_with('[') {
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
            if input.starts_with(base) {
                Some(base.len())
            } else {
                None
            }
        }
        Some(b'+') => {
            let mut pos = 0;
            while input.len() >= pos + base.len() && input[pos..].starts_with(base) {
                pos += base.len();
            }
            if pos > 0 { Some(pos) } else { None }
        }
        Some(b'*') => {
            let mut pos = 0;
            while input.len() >= pos + base.len() && input[pos..].starts_with(base) {
                pos += base.len();
            }
            Some(pos)
        }
        Some(b'?') => {
            if input.starts_with(base) {
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
    if pattern.is_empty() {
        return Some(0);
    }
    if pattern.len() == 1 {
        return match_pattern(input, pattern);
    }
    if pattern.trim() == "%NL;, ," || pattern == "\n, ," {
        return match_nl_comma_space_separator(input);
    }
    if delimiter_has_top_level_comma(pattern) {
        for alt in split_delimiter_alternatives(pattern) {
            if let Some(n) = match_delimiter_compound(input, &alt) {
                return Some(n);
            }
        }
        return None;
    }
    match_delimiter_compound(input, pattern)
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

fn match_delimiter_compound(input: &[u8], pattern: &str) -> Option<usize> {
    if pattern.is_empty() {
        return Some(0);
    }
    if pattern.len() == 1 {
        return match_pattern(input, pattern);
    }
    let mut pos = 0;
    for segment in split_delimiter_segments(pattern) {
        let matched = match_pattern(&input[pos..], segment)?;
        pos += matched;
    }
    Some(pos)
}

/// Split a separator/initiator/terminator into comma-separated alternatives.
/// Commas inside `%...;` entities and `[...]` classes are not separators.
/// Each comma in the list also denotes a literal `,` alternative (DFDL-12).
fn split_delimiter_alternatives(pattern: &str) -> alloc::vec::Vec<alloc::string::String> {
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
                alts.push(part.to_string());
            }
            alts.push(",".to_string());
            start = i + 1;
        }
        i += 1;
    }
    let tail = pattern[start..].trim();
    if !tail.is_empty() {
        alts.push(tail.to_string());
    }
    if alts.is_empty() {
        alts.push(pattern.to_string());
    }
    alts
}

/// Minimal bytes to emit for a delimiter on encode (one WSP for `+`, none for `*`/`?`).
pub fn encode_delimiter(pattern: &str) -> Vec<u8> {
    let pat = pattern.trim();
    if pat.is_empty() {
        return Vec::new();
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
    let seg = segment.trim();
    if seg.is_empty() {
        return Vec::new();
    }
    if seg.starts_with("%WSP") || seg.starts_with("%WS") {
        let q = seg.as_bytes().last().copied();
        return match q {
            Some(b'*') | Some(b'?') => Vec::new(),
            _ => vec![b' '],
        };
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
    if let Some(nl) = match_one_newline(input) {
        let mut total = nl;
        if input.get(total) == Some(&b',') {
            total += 1;
        }
        return Some(total);
    }
    if input.first() == Some(&b',') || input.first() == Some(&b' ') {
        return Some(1);
    }
    None
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
    fn match_bang_dot_bang_pattern() {
        use crate::schema::EncodingErrorPolicy;
        use crate::vm::encoding::read_one_utf8_char;
        let doc = b"!!\xc2\xc2!!";
        assert_eq!(match_length_pattern(doc, "!!.*!!"), Some(6));
        assert!(read_one_utf8_char(doc, 2, EncodingErrorPolicy::Error).is_err());
    }
}
