use crate::ir::{
    IrInputPathStep, IrInputValueCalcExpression, IrInputValueCalcSegment, StringId,
};
use crate::schema::{InputValueCalc, OutputValueCalc};
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub struct ValueCalcProps {
    pub input_value_calc: Option<InputValueCalc>,
    pub input_value_calc_literal: Option<StringId>,
    pub input_value_calc_sibling: Option<StringId>,
    pub input_value_calc_segments: Option<Vec<IrInputValueCalcSegment>>,
    pub input_value_calc_path: Option<Vec<IrInputPathStep>>,
    pub input_value_calc_expression: Option<IrInputValueCalcExpression>,
    pub output_value_calc: Option<OutputValueCalc>,
    pub output_value_calc_literal: Option<StringId>,
    pub output_value_calc_sibling: Option<StringId>,
    pub output_value_calc_path: Option<Vec<IrInputPathStep>>,
    pub output_value_calc_path_addend: Option<i64>,
    pub output_value_calc_scale: Option<i64>,
    pub output_value_calc_segments: Option<Vec<IrInputValueCalcSegment>>,
    pub output_value_calc_conditional: bool,
}

impl Default for ValueCalcProps {
    fn default() -> Self {
        Self {
            input_value_calc: None,
            input_value_calc_literal: None,
            input_value_calc_sibling: None,
            input_value_calc_segments: None,
            input_value_calc_path: None,
            input_value_calc_expression: None,
            output_value_calc: None,
            output_value_calc_literal: None,
            output_value_calc_sibling: None,
            output_value_calc_path: None,
            output_value_calc_path_addend: None,
            output_value_calc_scale: None,
            output_value_calc_segments: None,
            output_value_calc_conditional: false,
        }
    }
}
