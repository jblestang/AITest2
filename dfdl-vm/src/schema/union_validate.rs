use super::ast::{
    RestrictionBase, SchemaDocument, SimpleBase, TypeDef, TypeName, UnionMember,
};
use crate::schema::match_length_pattern;
use regex_automata::meta::Regex;
use regex_automata::{Anchored, Input};

fn pattern_matches_whole(text: &str, pat: &str) -> bool {
    let bytes = text.as_bytes();
    let pat = pat.trim();
    if pat.is_empty() {
        return text.is_empty();
    }
    let anchored = format!(r"\A(?:{pat})\z");
    if let Ok(re) = Regex::new(&anchored) {
        let input = Input::new(bytes).anchored(Anchored::Yes);
        return re.is_match(input);
    }
    match_length_pattern(bytes, pat).is_some_and(|len| len == bytes.len())
}

/// Return false when the lexical value is not valid for any member of the union underlying `type_name`.
pub fn validate_union_membership(
    schema: &SchemaDocument,
    type_name: &TypeName,
    text: &str,
) -> bool {
    let Some(TypeDef::Simple { base, .. }) = schema.resolve_type(type_name) else {
        return true;
    };
    let Some(members) = union_members_to_validate(schema, base) else {
        return true;
    };
    members
        .iter()
        .any(|member| union_member_accepts(schema, member, text))
}

fn union_members_to_validate<'a>(
    schema: &'a SchemaDocument,
    base: &'a SimpleBase,
) -> Option<&'a [UnionMember]> {
    match base {
        SimpleBase::Union { members } => Some(members.as_slice()),
        SimpleBase::Restriction {
            base: RestrictionBase::Named(name),
            ..
        } => {
            let parent = schema.types.get(name)?;
            let TypeDef::Simple { base: parent_base, .. } = parent else {
                return None;
            };
            match parent_base {
                SimpleBase::Union { members } => Some(members.as_slice()),
                SimpleBase::Restriction { .. } => union_members_to_validate(schema, parent_base),
                _ => None,
            }
        }
        _ => None,
    }
}

fn union_member_accepts(schema: &SchemaDocument, member: &UnionMember, text: &str) -> bool {
    match member {
        UnionMember::Named(name) => simple_type_accepts(schema, name, text),
        UnionMember::Inline(base) => simple_base_accepts(schema, base, text),
    }
}

fn simple_type_accepts(schema: &SchemaDocument, type_name: &TypeName, text: &str) -> bool {
    let Some(TypeDef::Simple { base, .. }) = schema.resolve_type(type_name) else {
        return false;
    };
    simple_base_accepts(schema, base, text)
}

fn simple_base_accepts(schema: &SchemaDocument, base: &SimpleBase, text: &str) -> bool {
    match base {
        SimpleBase::Union { members } => members
            .iter()
            .any(|member| union_member_accepts(schema, member, text)),
        SimpleBase::Restriction {
            base: parent,
            min_length,
            max_length,
            patterns,
            ..
        } => {
            if !restriction_level_accepts(*min_length, *max_length, patterns, text) {
                return false;
            }
            match parent {
                RestrictionBase::Builtin(_) => true,
                RestrictionBase::Named(name) => simple_type_accepts(schema, name, text),
            }
        }
        SimpleBase::Builtin(_) => true,
    }
}

fn restriction_level_accepts(
    min_length: Option<u64>,
    max_length: Option<u64>,
    patterns: &[alloc::string::String],
    text: &str,
) -> bool {
    let char_count = text.chars().count();
    if let Some(min) = min_length {
        if char_count < min as usize {
            return false;
        }
    }
    if let Some(max) = max_length {
        if char_count > max as usize {
            return false;
        }
    }
    if !patterns.is_empty() {
        let bytes = text.as_bytes();
        let mut any = false;
        for pat in patterns {
            if pattern_matches_whole(text, pat) {
                any = true;
                break;
            }
        }
        if !any {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::parse_schema;

    #[test]
    fn seven_as_not_in_union_of_length_branches() {
        let xsd = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:ex="http://example.com">
  <xs:simpleType name="uu">
    <xs:union>
      <xs:simpleType>
        <xs:restriction base="xs:string">
          <xs:maxLength value="1"/>
          <xs:minLength value="1"/>
        </xs:restriction>
      </xs:simpleType>
      <xs:simpleType>
        <xs:restriction base="xs:string">
          <xs:maxLength value="6"/>
          <xs:minLength value="2"/>
        </xs:restriction>
      </xs:simpleType>
    </xs:union>
  </xs:simpleType>
  <xs:simpleType name="restricted">
    <xs:restriction base="ex:uu">
      <xs:pattern value="a*"/>
    </xs:restriction>
  </xs:simpleType>
</xs:schema>"#;
        let doc = parse_schema(xsd).expect("parse");
        assert!(!validate_union_membership(
            &doc,
            &TypeName::new("restricted"),
            "aaaaaaa"
        ));
        assert!(validate_union_membership(&doc, &TypeName::new("restricted"), "aa"));
    }
}
