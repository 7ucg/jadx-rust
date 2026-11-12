#!/usr/bin/env bash
# Oracle: compare the classes of all code inputs (dex files anywhere in an archive, JVM classes in
# nested archives, the APKs of .xapk/.apks/.apkm packages) after the duplicate selection of the
# reference root, reference jadx (all input plugins) vs `jadx-rs input-classes`.
#
# Classes that the reference generates from the resource table (`R` classes) have the source `synthetic`
# and are not part of the comparison.
#
# Env:
#   JADX_REF   path to the Java jadx checkout (default: ../jadx)
#   JDK_BIN    directory with javac/java (default: from PATH)
#   WORK       scratch directory (default: ./work)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
JDK_BIN="${JDK_BIN:-}"
JAVAC="${JDK_BIN:+$JDK_BIN/}javac"; JAVA="${JDK_BIN:+$JDK_BIN/}java"
LIB="$JADX_REF/jadx-cli/build/install/jadx/lib/*"
RS="$ROOT/target/release/jadx-rs"

mkdir -p "$WORK/cls"
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml"
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT/tests/oracle/java/InputClassDump.java"

pass=0; fail=0
for f in "$@"; do
  "$JAVA" -Xss16m -Duser.language=en -cp "$WORK/cls:$LIB" InputClassDump "$f" 2>/dev/null \
    | grep -P '^CLS\t' | cut -f2- | LC_ALL=C sort > "$WORK/input.ref.txt" || true
  "$RS" input-classes "$f" 2>/dev/null | LC_ALL=C sort > "$WORK/input.rs.txt" || true
  n=$(wc -l < "$WORK/input.ref.txt")
  if cmp -s "$WORK/input.ref.txt" "$WORK/input.rs.txt"; then
    echo "OK    $n classes  $(basename "$f")"; pass=$((pass+1))
  else
    echo "DIFF  ref=$n rs=$(wc -l < "$WORK/input.rs.txt")  $(basename "$f")"
    diff "$WORK/input.ref.txt" "$WORK/input.rs.txt" | head -8
    fail=$((fail+1))
  fi
done
echo "parity: $pass/$((pass+fail)) inputs identical"
[ "$fail" -eq 0 ]
