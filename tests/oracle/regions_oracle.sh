#!/usr/bin/env bash
# Oracle: compare the region tree (and the instructions of its blocks) after the region stages of
# the reference with jadx-rs, per method (methods are sorted by id).
#
# STAGE: 12 RegionMakerVisitor, 13 IfRegionVisitor, 14 ReturnVisitor, 15 CleanRegions,
# 16 CodeShrinkVisitor, 17 CheckRegions, 18 LoopRegionVisitor, 19 SwitchBreakVisitor; from 20 on the
# reference runs its real pass list class by class (PipelineDump, see the stage table there), PIPELINE=1
# forces that mode for the lower stages too.
# The earlier stages (10 ModVisitor, 11 CodeShrinkVisitor, ...) run before, see blocks_oracle.sh.
#
# Env: JADX_REF (default ../jadx), JDK_BIN (dir with javac/java), WORK (default ./work),
# DIFF_ARGS (extra arguments of nodes_diff.py)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
JAVAC="${JDK_BIN:+$JDK_BIN/}javac"; JAVA="${JDK_BIN:+$JDK_BIN/}java"
LIB="$JADX_REF/jadx-cli/build/install/jadx/lib/*"
JOPTS="-Duser.language=en -Duser.country=US -Xss16m -Xmx12g"
STAGE="${STAGE:-12}"
export JADX_RS_STAGE="$STAGE"
mkdir -p "$WORK/cls"
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT/tests/oracle/java/InsnNodeDump.java" "$ROOT/tests/oracle/java/BlockDump.java" \
  "$ROOT/tests/oracle/java/RegionDump.java" "$ROOT/tests/oracle/java/RegionStages.java" \
  "$ROOT/tests/oracle/java/PipelineDump.java" "$ROOT/tests/oracle/java/ClassStateDump.java"
fail=0
for f in "$@"; do
  if [ "$STAGE" -ge 20 ] || [ -n "${PIPELINE:-}" ]; then
    # the real pass list, class by class
    "$JAVA" $JOPTS -cp "$WORK/cls:$LIB" PipelineDump "$f" "$STAGE" 2>/dev/null > "$WORK/regions.ref.raw" || true
  else
    "$JAVA" $JOPTS -cp "$WORK/cls:$LIB" BlockDump "$f" "$STAGE" 2>/dev/null > "$WORK/regions.ref.raw" || true
  fi
  python3 "$ROOT/tests/oracle/sort_methods.py" "$WORK/regions.ref.raw" > "$WORK/regions.ref.txt"
  "$ROOT/target/release/jadx-rs" insn-regions "$f" > "$WORK/regions.rs.raw" 2>/dev/null || true
  python3 "$ROOT/tests/oracle/sort_methods.py" "$WORK/regions.rs.raw" > "$WORK/regions.rs.txt"
  python3 "$ROOT/tests/oracle/nodes_diff.py" "$WORK/regions.ref.txt" "$WORK/regions.rs.txt" "$(basename "$f")" ${DIFF_ARGS:-} || fail=1
done
exit $fail
