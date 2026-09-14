mod infoset;
mod parser;
mod resources;
mod runner;
mod validation;

pub use resources::{load_tdml_resource, TdmlResourceContext};

pub use infoset::{
    compare_infoset, infer_root_element_name, infoset_xml_to_root_value, parse_expected_infoset_with_context,
    InfosetNode,
};
pub use parser::{
    effective_round_trip, effective_validation, parse_tdml, ParserTestCase, RoundTrip,
    TdmlDocument, TdmlSchema, TdmlSuite, TdmlValidationMode, UnparserTestCase,
};
pub use validation::{
    collect_post_decode_validation_errors, collect_post_decode_validation_errors_with_document,
};
pub use runner::{
    run_parser_test, run_parser_test_with_options, run_unparser_test, run_suite, ParserTestRunOptions,
    TestOutcome, TestResult,
};
