use super::helpers::should_emit_separator;
use crate::schema::SeparatorPosition;

#[test]
fn occurrence_postfix_emits_after_last_item() {
    assert!(should_emit_separator(
        SeparatorPosition::Postfix,
        2,
        3,
        true,
    ));
    assert!(should_emit_separator(
        SeparatorPosition::Postfix,
        2,
        3,
        false,
    ));
    assert!(!should_emit_separator(
        SeparatorPosition::Postfix,
        3,
        3,
        false,
    ));
}

#[test]
fn sibling_postfix_does_not_emit_after_last_child() {
    assert!(!should_emit_separator(
        SeparatorPosition::Postfix,
        0,
        1,
        false,
    ));
}

#[test]
fn infix_separator_skips_first_item() {
    assert!(!should_emit_separator(SeparatorPosition::Infix, 0, 3, true,));
    assert!(should_emit_separator(SeparatorPosition::Infix, 1, 3, true,));
}
