#!/usr/bin/env bash
# Fetch vendored Apache Daffodil TDML test resources (sparse checkout).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
git submodule update --init --depth 1 third_party/daffodil
cd third_party/daffodil
git sparse-checkout init --cone 2>/dev/null || true
git sparse-checkout set --skip-checks daffodil-test/src/test/resources
echo "Daffodil TDML files: $(find daffodil-test/src/test/resources -name '*.tdml' | wc -l)"
