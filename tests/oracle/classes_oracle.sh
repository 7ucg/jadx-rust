#!/usr/bin/env bash
# Oracle: compare the class list (type, flags, super, interfaces) produced by jadx-rs with the
# reference Java dex reader, for every input given on the command line.
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
[ -x "$RS" ] || cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml"
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT/tests/oracle/java/DexClassDump.java"

pass=0; fail=0
for f in "$@"; do
  "$JAVA" -cp "$WORK/cls:$LIB" DexClassDump "$f" 2>/dev/null | grep -P "^L[^\t]*;\t0x" > "$WORK/java.txt" || true
  "$RS" dex-classes "$f" 2>/dev/null > "$WORK/rust.txt" || true
  "$JAVA" -cp "$WORK/cls:$LIB" DexClassDump "$f" --counts 2>/dev/null | grep -P "^COUNTS" > "$WORK/java.counts" || true
  "$RS" dex-counts "$f" 2>/dev/null > "$WORK/rust.counts" || true
  cmp -s "$WORK/java.counts" "$WORK/rust.counts" || { echo "COUNTS DIFF $(basename "$f")"; diff "$WORK/java.counts" "$WORK/rust.counts" | head -4; fail=$((fail+1)); continue; }
  n=$(wc -l < "$WORK/java.txt")
  if cmp -s "$WORK/java.txt" "$WORK/rust.txt"; then
    echo "OK    $n classes  $(basename "$f")"; pass=$((pass+1))
  else
    echo "DIFF  java=$n rust=$(wc -l < "$WORK/rust.txt")  $(basename "$f")"
    diff "$WORK/java.txt" "$WORK/rust.txt" | head -5
    fail=$((fail+1))
  fi
done
echo "parity: $pass/$((pass+fail)) inputs identical"
[ "$fail" -eq 0 ]
