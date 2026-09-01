//! # dfdl-vm
//!
//! `no_std` + `alloc` DFDL virtual machine for Rust.
//!
//! Pipeline: **XSD + DFDL annotations → in-memory IR → VM encode/decode**.
//!
//! ```ignore
//! use dfdl_vm::{DfdlSpec, DfdlValue};
//! use alloc::collections::BTreeMap;
//!
//! let spec = DfdlSpec::from_xsd(include_str!("record.xsd"))?;
//! let bytes = &[0x00, 0x00, 0x00, 0x2A, 0x03];
//! let value = spec.decode(bytes)?;
//!
//! let mut fields = BTreeMap::new();
//! fields.insert("id".into(), DfdlValue::UnsignedInt(42));
//! fields.insert("flags".into(), DfdlValue::UnsignedByte(3));
//! let encoded = spec.encode(&DfdlValue::Sequence(fields))?;
//! ```

#![no_std]

extern crate alloc;

pub mod api;
pub mod error;
pub mod ir;
pub mod schema;
pub mod tdml;
pub mod value;
pub mod vm;

pub use api::{DfdlCodec, DfdlSchema, DfdlSpec};
pub use error::{Error, ParseError, Result, SchemaError, VmError};
pub use ir::{compile, IrProgram, IrProps};
pub use schema::{parse_schema, SchemaDocument};
pub use value::DfdlValue;
pub use vm::{Decoder, Encoder, RuntimeConfig};
pub use tdml::{parse_tdml, run_parser_test, run_suite, TestOutcome, TestResult};
