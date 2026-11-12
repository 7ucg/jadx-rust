#!/usr/bin/env bash
# Assemble tests/oracle/corpus/*.smali with the reference assembler, disassemble the result with
# the reference disassembler and compare with jadx-rs. With --update the committed fixtures
# (corpus.dex, corpus.expected.txt) are regenerated from the reference.
#
# Env: JADX_REF (default ../jadx), JDK_BIN (dir with javac/java), WORK (default ./work)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
JAVAC="${JDK_BIN:+$JDK_BIN/}javac"; JAVA="${JDK_BIN:+$JDK_BIN/}java"
LIB="$JADX_REF/jadx-cli/build/install/jadx/lib/*"
JOPTS="-Duser.language=en -Duser.country=US"
CORPUS="$ROOT/tests/oracle/corpus"

mkdir -p "$WORK/cls"
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT"/tests/oracle/java/SmaliAssemble.java "$ROOT"/tests/oracle/java/SmaliDump.java
"$JAVA" $JOPTS -cp "$WORK/cls:$LIB" SmaliAssemble "$CORPUS" "$WORK/corpus.dex"
"$JAVA" $JOPTS -cp "$WORK/cls:$LIB" SmaliDump "$WORK/corpus.dex" > "$WORK/corpus.ref.smali"
"$ROOT/target/release/jadx-rs" smali "$WORK/corpus.dex" > "$WORK/corpus.rs.smali"
if [ "${1:-}" = "--update" ]; then
  cp "$WORK/corpus.dex" "$CORPUS/corpus.dex"
  cp "$WORK/corpus.ref.smali" "$CORPUS/corpus.expected.txt"
fi
python3 "$ROOT/tests/oracle/smali_diff.py" "$WORK/corpus.ref.smali" "$WORK/corpus.rs.smali" 5
