mod infoset;
mod parser;
mod runner;
mod validation;

pub use infoset::{compare_infoset, infer_root_element_name, infoset_xml_to_root_value, InfosetNode};
pub use parser::{effective_round_trip, parse_tdml, ParserTestCase, RoundTrip, TdmlDocument, TdmlSchema, TdmlSuite, UnparserTestCase};
pub use validation::collect_post_decode_validation_errors;
pub use runner::{
    run_parser_test, run_parser_test_with_options, run_unparser_test, run_suite, ParserTestRunOptions,
    TestOutcome, TestResult,
};
