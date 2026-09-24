use crate::ir::{IrInputValueCalcSegment, StringId};
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FacetProps {
    pub facet_pattern_groups: Vec<StringId>,
    pub facet_enumeration: Vec<StringId>,
    pub value_min_inclusive: Option<i64>,
    pub value_max_inclusive: Option<i64>,
    pub value_min_exclusive: Option<i64>,
    pub value_max_exclusive: Option<i64>,
    pub value_min_inclusive_lexical: Option<StringId>,
    pub value_max_inclusive_lexical: Option<StringId>,
    pub value_min_exclusive_lexical: Option<StringId>,
    pub value_max_exclusive_lexical: Option<StringId>,
    pub total_digits: Option<u64>,
    pub fraction_digits: Option<u64>,
    pub facet_check_constraints: bool,
    pub assert_int_eq: Option<i64>,
    pub assert_eq_occurs_index_addend: Option<i64>,
    pub discriminator_test: Option<StringId>,
    pub facet_assert_message: Option<StringId>,
    pub facet_assert_message_segments: Option<Vec<IrInputValueCalcSegment>>,
    pub facet_assert_daffodil_prefix: bool,
}
