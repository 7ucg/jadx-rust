#!/usr/bin/env bash
# Oracle: compare the block graph after AttachTryCatch, ProcessInstructions and BlockSplitter,
# vs jadx-rs, per method (method blocks are sorted by id).
#
# Env: JADX_REF (default ../jadx), JDK_BIN (dir with javac/java), WORK (default ./work)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
JAVAC="${JDK_BIN:+$JDK_BIN/}javac"; JAVA="${JDK_BIN:+$JDK_BIN/}java"
LIB="$JADX_REF/jadx-cli/build/install/jadx/lib/*"
JOPTS="-Duser.language=en -Duser.country=US -Xss16m"
STAGE="${STAGE:-0}"
SUB="insn-blocks"; [ "$STAGE" != "0" ] && SUB="insn-cfg"
mkdir -p "$WORK/cls"
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT/tests/oracle/java/InsnNodeDump.java" "$ROOT/tests/oracle/java/BlockDump.java"
fail=0
for f in "$@"; do
  "$JAVA" $JOPTS -cp "$WORK/cls:$LIB" BlockDump "$f" "$STAGE" 2>/dev/null > "$WORK/blocks.ref.raw" || true
  python3 "$ROOT/tests/oracle/sort_methods.py" "$WORK/blocks.ref.raw" > "$WORK/blocks.ref.txt"
  "$ROOT/target/release/jadx-rs" $SUB "$f" > "$WORK/blocks.rs.raw" 2>/dev/null || true
  python3 "$ROOT/tests/oracle/sort_methods.py" "$WORK/blocks.rs.raw" > "$WORK/blocks.rs.txt"
  python3 "$ROOT/tests/oracle/nodes_diff.py" "$WORK/blocks.ref.txt" "$WORK/blocks.rs.txt" "$(basename "$f")" || fail=1
done
exit $fail
