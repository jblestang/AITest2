pub mod ast;
pub mod boolean_reps;
mod entities;
mod facets;
mod union_validate;
mod parser;
mod resolver;

pub(crate) use parser::{
    format_ref_qname_for_diag, lookup_named_escape_scheme_in_document,
    lookup_named_format_in_document, merge_dfdl_props, resolve_type_qname_in_schema,
};

pub use ast::*;
pub use union_validate::validate_union_membership;
pub use facets::{
    apply_effective_facets_to_ir, validate_facet_literals, validate_length_facets_for_type,
    EffectiveFacets,
};
pub use entities::{
    encode_delimiter, encode_delimiter_by_alt, encode_nl_comma_space_separator,
    encode_property_delimiter, encode_sequence_separator, expand_entities, match_delimiter_with_alt,
    validate_delimiter_es_restriction,
    expand_entities_for_encoding, expand_entities_str, is_nl_comma_space_pattern, match_delimiter,
    match_delimiter_opts,
    match_delimiter_opts_for_encoding, match_delimiter_with_alt_for_encoding,
    match_pattern_opts_for_encoding,
    parse_text_standard_separator_list,
    parse_text_standard_zero_rep_list,
    match_length_pattern, match_nl_comma_space_separator_with_flag, match_pattern,
    nil_value_alternatives,
    normalize_delimiter_pattern, parse_delimiter_literal_value, unescape_dfdl_open_braces,
    validate_delimiter_property_value, validate_delimiter_schema_attribute,
    delimited_terminator_expression_uses_es_literal, eval_discriminator_expression,
    eval_runtime_delimiter_expression,
    validate_runtime_delimiter_expression,
    runtime_delimiter_expression_may_be_zero_length, delimiter_alt_allows_trailing_input,
    is_zero_length_delimiter, validate_length_pattern,
    validate_dfdl_entities_in_property,
    validate_text_standard_distinct_values,
    validate_text_standard_exponent_rep_literal,
    validate_text_standard_separator_literal,
    validate_text_standard_special_value_literal,
    validate_text_standard_zero_rep_literal,
    validate_text_string_pad_character, validate_text_string_pad_character_compile,
    validate_text_string_pad_character_merged, validate_text_string_pad_character_runtime,
    validate_escape_block_property, validate_escape_character_property, validate_nil_value_compile,
    validate_text_number_pad_character_merged,
    validate_text_boolean_rep_value,
};
pub use parser::{
    get_global_element, parse_schema, parse_schema_with_options, parse_schema_with_resolver,
    resolve_global_element_storage_key, ParseOptions,
};
pub use resolver::SchemaResolver;
#[cfg(feature = "std")]
pub use resolver::read_schema_text_file;
