#!/usr/bin/env bash
# Oracle: compare the code string of every block instruction (and every wrapped instruction) as
# produced by the reference InsnGen after CodeShrinkVisitor, vs jadx-rs `insn-code`.
#
# NAMES=1: variables are declared at their first assignment and named by ApplyVariableNames
# (default: undeclared variables are printed as r<N>).
# REUSE_REF=1 keeps the reference output of a previous run (only the jadx-rs side is rebuilt).
# Env: JADX_REF (default ../jadx), JDK_BIN (dir with javac/java), WORK (default ./work)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
JAVAC="${JDK_BIN:+$JDK_BIN/}javac"; JAVA="${JDK_BIN:+$JDK_BIN/}java"
LIB="$JADX_REF/jadx-cli/build/install/jadx/lib/*"
JOPTS="-Duser.language=en -Duser.country=US -Xss16m -Xmx12g"
export NAMES="${NAMES:-0}"
mkdir -p "$WORK/cls"
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT/tests/oracle/java/InsnCodeDump.java"
fail=0
for f in "$@"; do
  if [ "${REUSE_REF:-0}" != "1" ]; then
    "$JAVA" $JOPTS -cp "$WORK/cls:$LIB" jadx.core.codegen.InsnCodeDump "$f" 2>/dev/null > "$WORK/code.ref.raw" || true
    python3 "$ROOT/tests/oracle/sort_code.py" "$WORK/code.ref.raw" > "$WORK/code.ref.txt"
  fi
  JADX_RS_STAGE=11 "$ROOT/target/release/jadx-rs" insn-code "$f" > "$WORK/code.rs.raw" 2>/dev/null || true
  python3 "$ROOT/tests/oracle/sort_code.py" "$WORK/code.rs.raw" > "$WORK/code.rs.txt"
  python3 "$ROOT/tests/oracle/code_diff.py" "$WORK/code.ref.txt" "$WORK/code.rs.txt" "$(basename "$f")" || fail=1
done
exit $fail
