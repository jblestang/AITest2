# dfdl-vm

`no_std` + `alloc` DFDL virtual machine for Rust.

**Pipeline:** XSD + DFDL annotations → in-memory IR → VM encode/decode

Build an encoder and decoder from a DFDL specification alone—no hand-written serializers.

## Architecture

```
┌─────────────┐     ┌──────────────┐     ┌─────────────────────────┐
│  XSD/DFDL   │ ──► │ Schema AST   │ ──► │ IrProgram (in-memory)   │
│  document   │     │ + properties │     │ Sequence/Choice/Element │
└─────────────┘     └──────────────┘     └───────────┬─────────────┘
                                                       │
                                                       ▼
                                           ┌───────────────────────┐
                                           │  DFDL VM              │
                                           │  Decoder / Encoder    │
                                           └───────────────────────┘
```

1. **XSD parser** — parses XML Schema with `dfdl:*` annotation properties
2. **IR builder** — compiles the schema AST into a flat graph of VM nodes
3. **VM** — walks IR nodes to decode bytes → `DfdlValue` or encode `DfdlValue` → bytes

## Usage

```rust
use dfdl_vm::{DfdlSpec, DfdlValue, DfdlCodec};
use alloc::collections::BTreeMap;

// From XSD string
let spec = DfdlSpec::from_xsd(include_str!("record.xsd"))?;

// Decode binary data
let value = spec.decode(&[0x00, 0x00, 0x00, 0x2A, 0x03])?;

// Encode back
let bytes = spec.encode(&value)?;

// Or use the codec wrapper
let codec = DfdlCodec::from_xsd(include_str!("record.xsd"))?;
let decoded = codec.decode(input)?;
let encoded = codec.encode(&decoded)?;

// Reusable encoder/decoder handles
let mut dec = spec.decoder();
let v1 = dec.decode(input_a)?;
let v2 = dec.decode(input_b)?;
```

## Supported DFDL (v1 subset)

| Feature | Status |
|---------|--------|
| `representation` binary / text | ✅ |
| `byteOrder` bigEndian / littleEndian | ✅ |
| `lengthKind` implicit, fixed, explicit, delimited, pattern, prefixed, endOfParent | ✅ |
| `xs:sequence`, `xs:choice` | ✅ |
| Initiator / terminator / separator | ✅ |
| Numeric, float, boolean, string, hexBinary, decimal, dateTime | ✅ |
| Bit-level fields, BCD / packed decimal | ✅ |
| `dfdl:format` defaults, nil handling, separator suppression | ✅ |
| Daffodil Section 12 lengthKind conformance (305 TDML cases) | ✅ |

## `no_std`

The crate uses `#![no_std]` with `extern crate alloc`. It has **zero required dependencies**.

```toml
[dependencies]
dfdl-vm = "0.1"
```

## Examples

See `tests/fixtures/` for sample XSD schemas:

- `record.xsd` — binary struct (u32 + u8)
- `text_message.xsd` — text format with fixed and delimited fields

Run tests:

```bash
cargo test -p dfdl-vm
```

## License

Apache-2.0
