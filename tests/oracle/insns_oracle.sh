#!/usr/bin/env bash
# Oracle: compare the normalised instruction stream (opcode, registers, literal, target,
# index, payloads) of every method, reference input plugin vs jadx-rs.
#
# Env: JADX_REF (default ../jadx), JDK_BIN (dir with javac/java), WORK (default ./work)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
JAVAC="${JDK_BIN:+$JDK_BIN/}javac"; JAVA="${JDK_BIN:+$JDK_BIN/}java"
LIB="$JADX_REF/jadx-cli/build/install/jadx/lib/*"
mkdir -p "$WORK/cls"
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT/tests/oracle/java/InsnDump.java"
fail=0
for f in "$@"; do
  "$JAVA" -cp "$WORK/cls:$LIB" InsnDump "$f" 2>/dev/null | grep -E "^(M|I) " > "$WORK/insn.ref.txt" || true
  "$ROOT/target/release/jadx-rs" insn-dump "$f" > "$WORK/insn.rs.txt" 2>/dev/null || true
  methods=$(grep -c "^M " "$WORK/insn.ref.txt" || true); insns=$(grep -c "^I " "$WORK/insn.ref.txt" || true)
  if cmp -s "$WORK/insn.ref.txt" "$WORK/insn.rs.txt"; then
    echo "OK    $methods methods, $insns insns  $(basename "$f")"
  else
    echo "DIFF  $(basename "$f")"; diff "$WORK/insn.ref.txt" "$WORK/insn.rs.txt" | head -6; fail=1
  fi
done
exit $fail
