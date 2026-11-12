#!/usr/bin/env bash
# Oracle: compare the block graph after AttachTryCatch, ProcessInstructions and BlockSplitter,
# vs jadx-rs, per method (method blocks are sorted by id).
#
# STAGE: 0 blocks, 1 block processing, 2 SSA, 3 move inline, 4 constructors, 5 code variables,
# 6 constant inlining, 7 type inference, 8 FixTypesVisitor, 9 FinishTypeInference, 10 ModVisitor,
# 11 CodeShrinkVisitor.
#
# Env: JADX_REF (default ../jadx), JDK_BIN (dir with javac/java), WORK (default ./work),
# JADX_RS_FINALLY=1 (run MarkFinallyVisitor after InitCodeVariables on both sides)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
JAVAC="${JDK_BIN:+$JDK_BIN/}javac"; JAVA="${JDK_BIN:+$JDK_BIN/}java"
LIB="$JADX_REF/jadx-cli/build/install/jadx/lib/*"
JOPTS="-Duser.language=en -Duser.country=US -Xss16m -Xmx${JAVA_XMX:-5g}"
# The finally extraction iterates hash maps whose keys contain an enum (identity hash code), so its
# result changes with unrelated changes of the running program. A constant identity hash makes the
# reference reproducible; the port emulates it.
if [ "${JADX_RS_FINALLY:-0}" = "1" ]; then JOPTS="$JOPTS -XX:+UnlockExperimentalVMOptions -XX:hashCode=2"; fi
STAGE="${STAGE:-0}"
SUB="insn-blocks"; [ "$STAGE" = "1" ] && SUB="insn-cfg"; [ "$STAGE" -ge 2 ] && SUB="insn-ssa"; export JADX_RS_STAGE="$STAGE"
mkdir -p "$WORK/cls"
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT/tests/oracle/java/InsnNodeDump.java" "$ROOT/tests/oracle/java/BlockDump.java" "$ROOT/tests/oracle/java/RegionDump.java" "$ROOT/tests/oracle/java/RegionStages.java"
fail=0
for f in "$@"; do
  "$JAVA" $JOPTS -cp "$WORK/cls:$LIB" BlockDump "$f" "$STAGE" 2>/dev/null > "$WORK/blocks.ref.raw" || true
  python3 "$ROOT/tests/oracle/sort_methods.py" "$WORK/blocks.ref.raw" > "$WORK/blocks.ref.txt"
  "$ROOT/target/release/jadx-rs" $SUB "$f" > "$WORK/blocks.rs.raw" 2>/dev/null || true
  python3 "$ROOT/tests/oracle/sort_methods.py" "$WORK/blocks.rs.raw" > "$WORK/blocks.rs.txt"
  python3 "$ROOT/tests/oracle/nodes_diff.py" "$WORK/blocks.ref.txt" "$WORK/blocks.rs.txt" "$(basename "$f")" || fail=1
done
exit $fail
