pub mod binary;
pub(crate) use binary::*;
pub mod bits;
pub(crate) use bits::*;
pub mod calendar_text;
pub(crate) use calendar_text::*;
pub mod cursor;
pub use cursor::Cursor;
pub mod delimited;
pub(crate) use delimited::*;
pub mod framed_payload;
pub(crate) use framed_payload::*;
pub mod nil;
pub(crate) use nil::*;
pub mod numeric_text_format;
pub(crate) use numeric_text_format::*;
pub mod numeric_text_parse;
pub(crate) use numeric_text_parse::*;
pub mod pad_trim;
pub(crate) use pad_trim::*;
pub mod property;
pub use property::resolve_output_new_line_for_encode;
pub(crate) use property::*;
pub mod scalar;
pub(crate) use scalar::*;
pub mod validate;
pub(crate) use validate::*;

use crate::ir::{IrProgram, IrProps, StringPool};
use crate::schema::BitOrder;
use alloc::collections::BTreeMap;
use alloc::string::String;

/// Runtime configuration shared by encoder and decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// When true, the decoder rejects input with leftover bytes after the root value.
    pub strict_eos: bool,
    /// When false, XSD facet checks are skipped (TDML `validation="off"`).
    pub enable_facet_validation: bool,
    /// When true, XSD facet checks run after parse (TDML `validationErrors` tests).
    pub defer_facet_validation: bool,
    /// When true, PUA code points in string values encode as UTF-8 (TDML text documents).
    pub encode_pua_codepoints_as_utf8: bool,
    /// TDML unparser tests: per-region transmission bit order while encoding.
    pub encode_tdml_bit_regions: Option<alloc::vec::Vec<(BitOrder, usize)>>,
    /// TDML `<daf:bind>` overrides for `dfdl:defineVariable` during decode.
    pub runtime_variable_overrides: BTreeMap<String, String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            strict_eos: true,
            enable_facet_validation: true,
            defer_facet_validation: false,
            encode_pua_codepoints_as_utf8: false,
            encode_tdml_bit_regions: None,
            runtime_variable_overrides: BTreeMap::new(),
        }
    }
}

pub(crate) fn encoding_name<'a>(
    props: &IrProps,
    strings: &'a StringPool,
) -> Result<&'a str, crate::error::VmError> {
    let raw = strings.get(props.encoding)?;
    Ok(crate::vm::encoding::resolve_encoding_with_byte_order(
        raw,
        props.byte_order,
    ))
}

pub(crate) struct VmContext<'a> {
    pub program: &'a IrProgram,
    pub config: RuntimeConfig,
}

impl<'a> VmContext<'a> {
    pub fn strings(&self) -> &StringPool {
        &self.program.strings
    }
}
