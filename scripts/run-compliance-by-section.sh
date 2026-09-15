#!/usr/bin/env bash
# Per-bucket TDML compliance (avoids daffodil_full_suite_report stack overflow).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TDML="$ROOT/third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil"
cd "$ROOT/dfdl-vm"
cargo build --test section_compliance_report -q

echo -e "Section\tPass\tFail\tSkip\tParseErr"
total_p=0 total_f=0 total_s=0 total_pe=0
for d in "$TDML"/*/; do
  section=$(basename "$d")
  line=$(DFDL_COMPLIANCE_SECTION="$section" cargo test -p dfdl-vm --test section_compliance_report compliance_section_snapshot -- --ignored --exact --nocapture 2>&1 | rg '^COMPLIANCE' || true)
  if [[ -z "$line" ]]; then
    echo -e "${section}\t-\t-\t-\tSTACK_OR_ERR"
    continue
  fi
  pass=$(sed -n 's/.*pass=\([0-9]*\).*/\1/p' <<<"$line")
  fail=$(sed -n 's/.*fail=\([0-9]*\).*/\1/p' <<<"$line")
  skip=$(sed -n 's/.*skip=\([0-9]*\).*/\1/p' <<<"$line")
  pe=$(sed -n 's/.*parse_fail=\([0-9]*\).*/\1/p' <<<"$line")
  echo -e "${section}\t${pass}\t${fail}\t${skip}\t${pe}"
  total_p=$((total_p + pass)); total_f=$((total_f + fail)); total_s=$((total_s + skip)); total_pe=$((total_pe + pe))
done
echo -e "TOTAL\t${total_p}\t${total_f}\t${total_s}\t${total_pe}"
