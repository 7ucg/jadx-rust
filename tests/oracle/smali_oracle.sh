#!/usr/bin/env bash
# Oracle: compare smali text of every class (jadx-rs vs the reference baksmali, debug info
# off) for each input given on the command line.
#
# Env: JADX_REF (default ../jadx), JDK_BIN (dir with javac/java), WORK (default ./work)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
JAVAC="${JDK_BIN:+$JDK_BIN/}javac"; JAVA="${JDK_BIN:+$JDK_BIN/}java"
LIB="$JADX_REF/jadx-cli/build/install/jadx/lib/*"
RS="$ROOT/target/release/jadx-rs"
JOPTS="-Duser.language=en -Duser.country=US"

mkdir -p "$WORK/cls"
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT/tests/oracle/java/SmaliDump.java"

fail=0
for f in "$@"; do
  "$JAVA" $JOPTS -cp "$WORK/cls:$LIB" SmaliDump "$f" > "$WORK/ref.smali" 2>/dev/null || true
  "$RS" smali "$f" > "$WORK/rs.smali" 2>/dev/null || true
  echo "== $(basename "$f")"
  python3 "$ROOT/tests/oracle/smali_diff.py" "$WORK/ref.smali" "$WORK/rs.smali" 3 | cut -c1-400 || fail=1
done
exit $fail
