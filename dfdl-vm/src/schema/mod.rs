pub mod ast;
mod entities;
mod parser;
mod resolver;

pub use ast::*;
pub use entities::{
    encode_delimiter, encode_delimiter_by_alt, encode_nl_comma_space_separator,
    encode_property_delimiter, encode_sequence_separator, expand_entities, match_delimiter_with_alt,
    validate_delimiter_es_restriction,
    expand_entities_str, is_nl_comma_space_pattern, match_delimiter, match_delimiter_opts,
    parse_text_standard_separator_list,
    parse_text_standard_zero_rep_list,
    match_length_pattern, match_nl_comma_space_separator_with_flag, match_pattern,
    normalize_delimiter_pattern, parse_delimiter_literal_value, unescape_dfdl_open_braces,
    validate_delimiter_property_value, is_zero_length_delimiter, validate_length_pattern,
    validate_dfdl_entities_in_property,
    validate_text_standard_distinct_values,
    validate_text_standard_exponent_rep_literal,
    validate_text_standard_separator_literal,
    validate_text_standard_special_value_literal,
    validate_text_standard_zero_rep_literal,
};
pub use parser::{parse_schema, parse_schema_with_resolver, ParseOptions};
pub use resolver::SchemaResolver;
