#!/usr/bin/env bash
# Oracle: decode the text resources (AndroidManifest.xml, binary xml files, resources.arsc ->
# res/values*/*.xml + public.xml) and the resource id name table of APKs with the reference
# decompiler (tests/oracle/java/ResDump.java) and with `jadx-rs res-dump`, then compare the two
# output trees byte for byte. The class alias table (renamed and inner classes referenced by
# name in the XML) and the names of the app constants that carry resource ids are taken from
# the reference and given to both sides.
#
# Usage: res_oracle.sh file.apk...
# Env: JADX_REF (default ../jadx), JDK_BIN (dir with javac/java), WORK (default ./work)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
JAVAC="${JDK_BIN:+$JDK_BIN/}javac"; JAVA="${JDK_BIN:+$JDK_BIN/}java"
LIB="$JADX_REF/jadx-cli/build/install/jadx/lib/*"
JOPTS="-Duser.language=en -Duser.country=US -Xmx8g"
mkdir -p "$WORK/cls"
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT/tests/oracle/java/ResDump.java"
fail=0
n=0
for f in "$@"; do
  n=$((n + 1))
  ref="$WORK/res.ref.$n"; rs="$WORK/res.rs.$n"; aux="$WORK/res.aux.$n"
  rm -rf "$ref" "$rs" "$aux"; mkdir -p "$ref" "$rs" "$aux"
  "$JAVA" $JOPTS -cp "$WORK/cls:$LIB" ResDump "$f" "$ref" "$aux" >/dev/null 2>&1 || true
  "$ROOT/target/release/jadx-rs" res-dump "$f" "$rs" "$aux" 2>/dev/null || true
  python3 "$ROOT/tests/oracle/res_diff.py" "$ref" "$rs" "input $n" || fail=1
done
exit $fail
