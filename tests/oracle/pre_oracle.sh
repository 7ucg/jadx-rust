#!/usr/bin/env bash
# Oracle: compare the class level state after the pre decompile passes (access flags, anonymous
# class marks, usage info, dependencies, const storage) of the reference vs `jadx-rs pre-info`.
#
# Env: JADX_REF (default ../jadx), JDK_BIN (dir with javac/java), WORK (default ./work)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
JAVAC="${JDK_BIN:+$JDK_BIN/}javac"; JAVA="${JDK_BIN:+$JDK_BIN/}java"
LIB="$JADX_REF/jadx-cli/build/install/jadx/lib/*"
JOPTS="-Duser.language=en -Duser.country=US -Xss16m -Xmx4g"
mkdir -p "$WORK/cls"
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT/tests/oracle/java/ClassInfoDump.java"
fail=0
for f in "$@"; do
  "$JAVA" $JOPTS -cp "$WORK/cls:$LIB" ClassInfoDump "$f" 2>/dev/null > "$WORK/pre.ref.txt" || true
  "$ROOT/target/release/jadx-rs" pre-info "$f" > "$WORK/pre.rs.txt" 2>/dev/null || true
  python3 "$ROOT/tests/oracle/pre_info_diff.py" "$WORK/pre.ref.txt" "$WORK/pre.rs.txt" "$(basename "$f")" || fail=1
done
exit $fail
