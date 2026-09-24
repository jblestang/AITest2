use crate::ir::{IrInputPathStep, IrPrefixLength, StringId};
use crate::schema::{LengthKind, LengthUnits, OccursCountKind};

#[derive(Debug, Clone, PartialEq)]
pub struct LengthProps {
    pub length_kind: LengthKind,
    pub length_kind_defined: bool,
    pub length: Option<u64>,
    pub length_sibling: Option<StringId>,
    pub length_sibling_cast_long: bool,
    pub length_sibling_adjust: i64,
    pub length_expr_unparsed: bool,
    pub length_self_string_max_cap: Option<u64>,
    pub length_self_value_length: bool,
    pub length_units: LengthUnits,
    pub length_pattern: Option<StringId>,
    pub occurs_min: u64,
    pub occurs_max: Option<u64>,
    pub occurs_count_kind: OccursCountKind,
    pub occurs_count_fn_path: Option<alloc::vec::Vec<IrInputPathStep>>,
    pub occurs_count_expr: Option<StringId>,
    pub prefix_length: Option<alloc::boxed::Box<IrPrefixLength>>,
    pub prefix_includes_prefix_length: bool,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub facet_length: Option<u64>,
    pub implicit_facet_length: Option<u64>,
}

impl Default for LengthProps {
    fn default() -> Self {
        Self {
            length_kind: LengthKind::Implicit,
            length_kind_defined: false,
            length: None,
            length_sibling: None,
            length_sibling_cast_long: false,
            length_sibling_adjust: 0,
            length_expr_unparsed: false,
            length_self_string_max_cap: None,
            length_self_value_length: false,
            length_units: LengthUnits::Bytes,
            length_pattern: None,
            occurs_min: 1,
            occurs_max: Some(1),
            occurs_count_kind: OccursCountKind::Implicit,
            occurs_count_fn_path: None,
            occurs_count_expr: None,
            prefix_length: None,
            prefix_includes_prefix_length: false,
            min_length: None,
            max_length: None,
            facet_length: None,
            implicit_facet_length: None,
        }
    }
}
