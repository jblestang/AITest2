use crate::ir::StringId;
use crate::schema::{BinaryFloatRep, BinaryNumberCheckPolicy, BinaryNumberRep};

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryProps {
    pub binary_number_rep: BinaryNumberRep,
    pub binary_packed_sign_codes: StringId,
    pub binary_packed_sign_codes_defined: bool,
    pub binary_number_check_policy: BinaryNumberCheckPolicy,
    pub binary_calendar_rep: BinaryNumberRep,
    pub binary_calendar_epoch: Option<StringId>,
    pub binary_float_rep: BinaryFloatRep,
    pub binary_decimal_virtual_point: u32,
    pub binary_decimal_virtual_point_signed: Option<i32>,
    pub decimal_signed: bool,
    pub binary_boolean_true_rep: Option<u64>,
    pub binary_boolean_true_rep_defined: bool,
    pub binary_boolean_false_rep: Option<u64>,
    pub binary_boolean_false_rep_defined: bool,
}

impl Default for BinaryProps {
    fn default() -> Self {
        Self {
            binary_number_rep: BinaryNumberRep::Binary,
            binary_packed_sign_codes: StringId(0),
            binary_packed_sign_codes_defined: false,
            binary_number_check_policy: BinaryNumberCheckPolicy::Lax,
            binary_calendar_rep: BinaryNumberRep::Binary,
            binary_calendar_epoch: None,
            binary_float_rep: BinaryFloatRep::Ieee,
            binary_decimal_virtual_point: 0,
            binary_decimal_virtual_point_signed: None,
            decimal_signed: true,
            binary_boolean_true_rep: None,
            binary_boolean_true_rep_defined: false,
            binary_boolean_false_rep: None,
            binary_boolean_false_rep_defined: false,
        }
    }
}
