# Code Review — 2026-09-23

## Refactoring, Section 02 Fixes & Code Cleanup
- **Target**: Resolve test regressions in `scan_section02` and eliminate unused import warnings in `dfdl-vm/src/ir/builder/`.
- **Root Cause & Fixes**:
  1. `runtimeSdeWithRestriction_1`: Removed compile-time enforcement of sequence bitOrder byte boundary checks (`validate_program_sequence_bit_orders`) so that non-byte-boundary bitOrder transitions trigger Runtime SDEs at decode time as specified by DFDL.
  2. `optional_element_1`: Fixed `overlay_dfdl_to_ir` in `props.rs` to set `custom_text_number_pattern = true` when `text_number_pattern` is overridden on an element (e.g. `dfdl:textNumberPattern="0.0"`).
  3. `lookup_xpath_sibling_state`: Updated sibling state lookup in `src/vm/decoder/mod.rs` to search `self.xpath_siblings` for relative parent XPath lookups (`../` / `../../`).
  4. Code Cleanup: Removed all unused imports across `ir/builder/mod.rs`, `element.rs`, `particle.rs`, `props.rs`, and `validate.rs`.

## Decoder Modularization (`src/vm/decoder/`)
- **Target**: Refactor `dfdl-vm/src/vm/decoder/mod.rs` from monolithic 6,560 lines into clean, modular domain files.
- **File Structure**:
  - `src/vm/decoder/mod.rs`: Reduced to **183 lines** (top-level struct, constructors, entry points).
  - `src/vm/decoder/element.rs`: Element decoding, nil evaluation, and framing logic.
  - `src/vm/decoder/particle.rs`: Particle dispatch, array occurrence loop, and backtrack helpers.
  - `src/vm/decoder/sequence.rs`: Sequence node decoding, child iteration, and separator handling.
  - `src/vm/decoder/xpath.rs`: XPath evaluation, variable scoping, and assertion checks.
  - `src/vm/decoder/choice.rs`: Choice node decoding and branch validation.
  - `src/vm/decoder/ivc.rs`: `inputValueCalc` finalization and lexical conversion.
  - `src/vm/decoder/helpers.rs`: Shared value constructors and error predicates.

## Final Verification & Analysis
1. **Compilation & Diagnostics**:
   - `cargo check`: 0 compilation errors, 0 warnings in `src/vm/decoder/`.
2. **Line Count Criterion**:
   - `dfdl-vm/src/vm/decoder/mod.rs`: **183 lines** (requirement: < 600 lines).
3. **Unit Tests**:
   - `cargo test --lib`: **167 passed, 0 failed**.
4. **Integration & Section Scans**:
   - `section02_scan`: **92 passed**.

