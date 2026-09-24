pub mod ast;
pub mod boolean_reps;
mod entities;
mod facets;
mod parser;
mod resolver;
mod union_validate;

pub(crate) use parser::{
    lookup_named_escape_scheme_in_document, merge_dfdl_props, resolve_type_qname_in_schema,
};

pub use ast::*;
pub use entities::{
    delimited_terminator_expression_uses_es_literal, delimiter_alt_allows_trailing_input,
    delimiter_alternatives, delimiter_match_len_at, encode_delimiter, encode_delimiter_by_alt,
    encode_delimiter_for_encoding, encode_framing_property_literal,
    encode_nl_comma_space_separator, encode_property_delimiter,
    encode_property_delimiter_for_encoding, encode_sequence_separator,
    eval_discriminator_expression, eval_path_indexed_delimiter_expression,
    eval_runtime_delimiter_expression, expand_entities, expand_entities_for_encoding,
    expand_entities_str, extra_escaped_characters_from_property, is_nl_comma_space_pattern,
    is_zero_length_delimiter, match_delimiter, match_delimiter_opts,
    match_delimiter_opts_for_encoding, match_delimiter_with_alt,
    match_delimiter_with_alt_for_encoding, match_length_pattern,
    match_nl_comma_space_separator_with_flag, match_pattern, match_pattern_opts_for_encoding,
    nil_value_alternatives, normalize_delimiter_pattern, parse_delimiter_literal_value,
    parse_text_standard_separator_list, parse_text_standard_zero_rep_list,
    runtime_delimiter_expression_may_be_zero_length, unescape_dfdl_open_braces,
    unquote_xpath_string_literal, validate_delimiter_es_restriction,
    validate_delimiter_property_value, validate_delimiter_schema_attribute,
    validate_dfdl_entities_in_property, validate_escape_block_property,
    validate_escape_character_property, validate_extra_escaped_characters_property,
    validate_length_pattern, validate_nil_value_compile, validate_runtime_delimiter_expression,
    validate_text_boolean_rep_value, validate_text_number_pad_character_merged,
    validate_text_standard_distinct_values, validate_text_standard_exponent_rep_literal,
    validate_text_standard_separator_literal, validate_text_standard_special_value_literal,
    validate_text_standard_zero_rep_literal, validate_text_string_pad_character,
    validate_text_string_pad_character_compile, validate_text_string_pad_character_merged,
    validate_text_string_pad_character_runtime,
};
pub use facets::{
    apply_effective_facets_to_ir, validate_facet_literals, validate_length_facets_for_type,
    EffectiveFacets,
};
pub(crate) use parser::parse_input_value_calc_expression;
pub use parser::{
    format_local_from_storage_key, format_storage_key, get_global_element,
    get_global_element_error, parse_schema, parse_schema_with_options, parse_schema_with_resolver,
    resolve_global_element_storage_key, ParseOptions,
};
#[cfg(feature = "std")]
pub use resolver::read_schema_text_file;
pub use resolver::SchemaResolver;
pub use union_validate::validate_union_membership;
