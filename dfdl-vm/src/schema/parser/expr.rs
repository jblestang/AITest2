use super::props::parse_infoset_path_step;
use crate::schema::{InputValueCalcExpression, IvcXsCast};
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) fn parse_xs_string_literal_arg(arg: &str) -> Option<String> {
    let arg = arg.trim();
    if arg.len() < 2 || !arg.starts_with('\'') || !arg.ends_with('\'') {
        return None;
    }
    Some(arg[1..arg.len() - 1].to_string())
}

pub(crate) fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim().to_string());
    out
}

pub(crate) fn split_top_level_ivc_op(s: &str, op: char) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            c if c == op && depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    if parts.len() <= 1 {
        return None;
    }
    Some(parts)
}

pub(crate) fn parse_ivc_path_steps(
    s: &str,
) -> Option<(bool, Vec<(Option<String>, String, Option<u32>, bool)>)> {
    let s = s.trim();
    let (parent_root, rest) = if let Some(r) = s.strip_prefix("parent::") {
        (true, r)
    } else if let Some(r) = s.strip_prefix('/') {
        (false, r)
    } else {
        let r = s.strip_prefix("../").or_else(|| s.strip_prefix("..\\"))?;
        (false, r)
    };
    if rest.is_empty() {
        return None;
    }
    let mut steps = Vec::new();
    for step in rest.split('/').filter(|p| !p.is_empty()) {
        steps.push(parse_infoset_path_step(step));
    }
    Some((parent_root, steps))
}

pub(crate) fn parse_ivc_xs_cast_kind(prefix: &str) -> Option<IvcXsCast> {
    use IvcXsCast::*;
    Some(match prefix {
        "xs:byte" => Byte,
        "xs:short" => Short,
        "xs:int" => Int,
        "xs:long" => Long,
        "xs:unsignedByte" => UnsignedByte,
        "xs:unsignedShort" => UnsignedShort,
        "xs:unsignedInt" => UnsignedInt,
        "xs:unsignedLong" => UnsignedLong,
        "xs:float" => Float,
        "xs:double" => Double,
        "xs:string" => String,
        _ => return None,
    })
}

enum IvcIntegerLexical {
    I64(i64),
    Wide(String),
}

fn parse_ivc_integer_lexical(s: &str) -> Option<IvcIntegerLexical> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let rest = if let Some(r) = s.strip_prefix('+').or_else(|| s.strip_prefix('-')) {
        r
    } else {
        s
    };
    if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if let Ok(v) = s.parse::<i64>() {
        return Some(IvcIntegerLexical::I64(v));
    }
    Some(IvcIntegerLexical::Wide(s.to_string()))
}

pub(crate) fn split_top_level_ivc_div(s: &str) -> Option<Vec<String>> {
    let s = s.trim();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        match ch {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(ch);
                i += 1;
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(ch);
                i += 1;
            }
            _ if depth == 0 && s[i..].starts_with(" div ") => {
                parts.push(current.trim().to_string());
                current.clear();
                i += 5;
            }
            _ => {
                current.push(ch);
                i += 1;
            }
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    if parts.len() <= 1 {
        return None;
    }
    Some(parts)
}

pub(crate) fn extract_ivc_paren_argument(s: &str, open_prefix: &str) -> Option<String> {
    let rest = s.strip_prefix(open_prefix)?;
    let mut depth = 1i32;
    for (i, ch) in rest.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(rest[..i].trim().to_string());
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn parse_ivc_path_expr(s: &str) -> Option<InputValueCalcExpression> {
    let (parent_root, steps) = parse_ivc_path_steps(s)?;
    Some(InputValueCalcExpression::Path { parent_root, steps })
}

pub(crate) fn parse_ivc_primary(s: &str) -> Option<InputValueCalcExpression> {
    let s = s.trim();
    if let Some(arg) = extract_ivc_paren_argument(s, "fn:ceiling(") {
        let inner = parse_ivc_add_expr(&arg)?;
        return Some(InputValueCalcExpression::Ceiling(Box::new(inner)));
    }
    for (prefix, kind) in [
        ("xs:byte(", IvcXsCast::Byte),
        ("xsd:byte(", IvcXsCast::Byte),
        ("xs:short(", IvcXsCast::Short),
        ("xsd:short(", IvcXsCast::Short),
        ("xs:int(", IvcXsCast::Int),
        ("xsd:int(", IvcXsCast::Int),
        ("xs:long(", IvcXsCast::Long),
        ("xsd:long(", IvcXsCast::Long),
        ("xs:unsignedByte(", IvcXsCast::UnsignedByte),
        ("xsd:unsignedByte(", IvcXsCast::UnsignedByte),
        ("xs:unsignedShort(", IvcXsCast::UnsignedShort),
        ("xsd:unsignedShort(", IvcXsCast::UnsignedShort),
        ("xs:unsignedInt(", IvcXsCast::UnsignedInt),
        ("xsd:unsignedInt(", IvcXsCast::UnsignedInt),
        ("xs:unsignedLong(", IvcXsCast::UnsignedLong),
        ("xsd:unsignedLong(", IvcXsCast::UnsignedLong),
        ("xs:float(", IvcXsCast::Float),
        ("xsd:float(", IvcXsCast::Float),
        ("xs:double(", IvcXsCast::Double),
        ("xsd:double(", IvcXsCast::Double),
        ("xs:string(", IvcXsCast::String),
        ("xsd:string(", IvcXsCast::String),
    ] {
        if let Some(arg) = extract_ivc_paren_argument(s, prefix) {
            let inner = parse_ivc_add_expr(&arg)?;
            return Some(InputValueCalcExpression::Cast {
                kind,
                inner: Box::new(inner),
            });
        }
    }
    if let Some(rest) = s.strip_prefix('-') {
        if let Some(lit) = parse_ivc_integer_lexical(rest) {
            return Some(match lit {
                IvcIntegerLexical::I64(v) => InputValueCalcExpression::Literal(-v),
                IvcIntegerLexical::Wide(text) => {
                    let mut neg = String::from("-");
                    neg.push_str(text.trim_start_matches('+'));
                    InputValueCalcExpression::LiteralLexical(neg)
                }
            });
        }
    }
    if let Some(lit) = parse_ivc_integer_lexical(s) {
        return Some(match lit {
            IvcIntegerLexical::I64(v) => InputValueCalcExpression::Literal(v),
            IvcIntegerLexical::Wide(text) => InputValueCalcExpression::LiteralLexical(text),
        });
    }
    if s.len() >= 2
        && ((s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')))
    {
        let inner = &s[1..s.len() - 1];
        let unescaped = inner.replace("''", "'").replace("\"\"", "\"");
        return Some(InputValueCalcExpression::LiteralLexical(unescaped));
    }
    if let Some(name) = s.strip_prefix('$').map(str::trim) {
        if !name.is_empty() && !name.contains(' ') {
            return Some(InputValueCalcExpression::Variable(name.to_string()));
        }
    }
    parse_ivc_path_expr(s)
}

pub(crate) fn parse_ivc_unary_expr(s: &str) -> Option<InputValueCalcExpression> {
    parse_ivc_primary(s)
}

pub(crate) fn parse_ivc_div_expr(s: &str) -> Option<InputValueCalcExpression> {
    let s = s.trim();
    if let Some(parts) = split_top_level_ivc_div(s) {
        let mut left = parse_ivc_unary_expr(&parts[0])?;
        for part in parts.iter().skip(1) {
            let right = parse_ivc_unary_expr(part)?;
            left = InputValueCalcExpression::Div(Box::new(left), Box::new(right));
        }
        return Some(left);
    }
    parse_ivc_unary_expr(s)
}

pub(crate) fn split_top_level_ivc_sub(s: &str) -> Option<Vec<String>> {
    let s = s.trim();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        match ch {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(ch);
                i += 1;
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(ch);
                i += 1;
            }
            _ if depth == 0 && s[i..].starts_with(" - ") => {
                parts.push(current.trim().to_string());
                current.clear();
                i += 3;
            }
            _ => {
                current.push(ch);
                i += 1;
            }
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    if parts.len() <= 1 {
        return None;
    }
    Some(parts)
}

pub(crate) fn parse_ivc_mul_expr(s: &str) -> Option<InputValueCalcExpression> {
    let s = s.trim();
    if let Some(parts) = split_top_level_ivc_op(s, '*') {
        let mut terms = Vec::new();
        for part in parts {
            terms.push(parse_ivc_div_expr(&part)?);
        }
        return Some(InputValueCalcExpression::Mul(terms));
    }
    parse_ivc_div_expr(s)
}

pub(crate) fn parse_ivc_sub_expr(s: &str) -> Option<InputValueCalcExpression> {
    let s = s.trim();
    if let Some(parts) = split_top_level_ivc_sub(s) {
        let mut left = parse_ivc_mul_expr(&parts[0])?;
        for part in parts.iter().skip(1) {
            let right = parse_ivc_mul_expr(part)?;
            left = InputValueCalcExpression::Sub(Vec::from([left, right]));
        }
        return Some(left);
    }
    parse_ivc_mul_expr(s)
}

pub(crate) fn parse_ivc_add_expr(s: &str) -> Option<InputValueCalcExpression> {
    let s = s.trim();
    if let Some(parts) = split_top_level_ivc_op(s, '+') {
        let mut terms = Vec::new();
        for part in parts {
            terms.push(parse_ivc_sub_expr(&part)?);
        }
        return Some(InputValueCalcExpression::Add(terms));
    }
    parse_ivc_sub_expr(s)
}
