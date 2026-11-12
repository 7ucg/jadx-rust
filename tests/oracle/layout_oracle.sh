#!/usr/bin/env bash
# Compare the output file layout (directory and file names, no contents) of `jadx-rs -d` with the
# reference decompiler for one or more inputs. Default input: the corpus dex.
#
# Usage: layout_oracle.sh [input...]
# Env: JADX_REF (default ../jadx), JAVA_HOME (JDK of the reference), WORK (default ./work)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
REF_BIN="$JADX_REF/jadx-cli/build/install/jadx/bin/jadx"
INPUTS=("$@")
[ ${#INPUTS[@]} -gt 0 ] || INPUTS=("$ROOT/tests/oracle/corpus/corpus.dex")

cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
status=0
for input in "${INPUTS[@]}"; do
  name="$(basename "$input")"
  rm -rf "$WORK/layout/ref/$name" "$WORK/layout/rs/$name"
  mkdir -p "$WORK/layout/ref/$name" "$WORK/layout/rs/$name"
  # the reference exits non-zero if some classes had errors, the layout is still written
  "$REF_BIN" -q -d "$WORK/layout/ref/$name" "$input" >/dev/null 2>&1 || true
  "$ROOT/target/release/jadx-rs" -q -d "$WORK/layout/rs/$name" "$input" >/dev/null 2>&1 || true
  (cd "$WORK/layout/ref/$name" && find . | sort) > "$WORK/layout/ref/$name.lst"
  (cd "$WORK/layout/rs/$name" && find . | sort) > "$WORK/layout/rs/$name.lst"
  if diff -u "$WORK/layout/ref/$name.lst" "$WORK/layout/rs/$name.lst"; then
    echo "layout OK: $name ($(wc -l < "$WORK/layout/rs/$name.lst") entries)"
  else
    echo "layout DIFFERS: $name"
    status=1
  fi
done
exit $status
