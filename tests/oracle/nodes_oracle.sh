#!/usr/bin/env bash
# Oracle: compare decoded instruction nodes (InsnDecoder + ProcessInstructions), reference
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
mkdir -p "$WORK/cls"
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT/tests/oracle/java/InsnNodeDump.java"
fail=0
for f in "$@"; do
  "$JAVA" $JOPTS -cp "$WORK/cls:$LIB" InsnNodeDump "$f" 2>/dev/null > "$WORK/nodes.ref.raw" || true
  python3 "$ROOT/tests/oracle/sort_methods.py" "$WORK/nodes.ref.raw" > "$WORK/nodes.ref.txt"
  "$ROOT/target/release/jadx-rs" insn-nodes "$f" > "$WORK/nodes.rs.raw" 2>/dev/null || true
  python3 "$ROOT/tests/oracle/sort_methods.py" "$WORK/nodes.rs.raw" > "$WORK/nodes.rs.txt"
  python3 "$ROOT/tests/oracle/nodes_diff.py" "$WORK/nodes.ref.txt" "$WORK/nodes.rs.txt" "$(basename "$f")" || fail=1
done
exit $fail
