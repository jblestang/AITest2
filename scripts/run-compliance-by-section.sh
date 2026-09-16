#!/usr/bin/env bash
# Per-bucket TDML compliance (avoids daffodil_full_suite_report stack overflow).
#
# Usage:
#   ./scripts/run-compliance-by-section.sh
#   RELEASE=1 TIMEOUT_SECS=20 ./scripts/run-compliance-by-section.sh
#   OUT=artifacts/compliance.tsv ./scripts/run-compliance-by-section.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TDML="$ROOT/third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil"
RELEASE="${RELEASE:-1}"
TIMEOUT_SECS="${TIMEOUT_SECS:-20}"
OUT="${OUT:-}"

cd "$ROOT/dfdl-vm"
if [[ "$RELEASE" == "1" ]]; then
  cargo build --release --test section_compliance_report -q
  CARGO=(cargo test -p dfdl-vm --release --test section_compliance_report)
else
  cargo build --test section_compliance_report -q
  CARGO=(cargo test -p dfdl-vm --test section_compliance_report)
fi

emit() {
  if [[ -n "$OUT" ]]; then
    echo -e "$1" >>"$OUT"
  fi
  echo -e "$1"
}

if [[ -n "$OUT" ]]; then
  mkdir -p "$(dirname "$OUT")"
  : >"$OUT"
fi

emit "Section\tPass\tFail\tSkip\tParseErr\tStatus"
total_p=0 total_f=0 total_s=0 total_pe=0
for d in "$TDML"/*/; do
  section=$(basename "$d")
  set +e
  combined="$(
    timeout "$TIMEOUT_SECS" env DFDL_COMPLIANCE_SECTION="$section" \
      "${CARGO[@]}" compliance_section_snapshot -- --ignored --exact --nocapture 2>&1
  )"
  status=$?
  set -e
  line=$(grep -E '^COMPLIANCE' <<<"$combined" | tail -1 || true)
  if [[ -n "$line" ]]; then
    # Use tab-boundary patterns so `parse_fail=` is not mistaken for `fail=`.
    pass=$(sed -n 's/.*\tpass=\([0-9]*\).*/\1/p' <<<"$line")
    fail=$(sed -n 's/.*\tfail=\([0-9]*\).*/\1/p' <<<"$line")
    skip=$(sed -n 's/.*\tskip=\([0-9]*\).*/\1/p' <<<"$line")
    pe=$(sed -n 's/.*parse_fail=\([0-9]*\).*/\1/p' <<<"$line")
    emit "${section}\t${pass}\t${fail}\t${skip}\t${pe}\tok"
    total_p=$((total_p + pass))
    total_f=$((total_f + fail))
    total_s=$((total_s + skip))
    total_pe=$((total_pe + pe))
  elif [[ "$status" -eq 124 ]]; then
    emit "${section}\t-\t-\t-\t-\ttimeout_${TIMEOUT_SECS}s"
  else
    emit "${section}\t-\t-\t-\t-\tSTACK_OR_ERR"
  fi
done
emit "TOTAL\t${total_p}\t${total_f}\t${total_s}\t${total_pe}\t(partial if timeouts)"
