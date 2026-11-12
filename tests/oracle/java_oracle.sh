#!/usr/bin/env bash
# Oracle: compare the class data and the normalised instruction stream of .class/.jar inputs,
# reference java input plugin vs jadx-rs. Usage:
#
#   java_oracle.sh [--corpus] [--classes|--insns] [file ...]
#
# With --corpus the Java sources under tests/oracle/corpus/java are compiled (with and without
# debug info, plus a jar nested in a jar) and added to the inputs.
#
# Env: JADX_REF (default ../jadx), JDK_BIN (dir with javac/java/jar), WORK (default ./work)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
JAVAC="${JDK_BIN:+$JDK_BIN/}javac"; JAVA="${JDK_BIN:+$JDK_BIN/}java"; JAR="${JDK_BIN:+$JDK_BIN/}jar"
LIB="$JADX_REF/jadx-cli/build/install/jadx/lib/*"
RS="$ROOT/target/release/jadx-rs"
mode=both; corpus=0; inputs=()
for a in "$@"; do
  case "$a" in
    --corpus) corpus=1 ;;
    --classes) mode=classes ;;
    --insns) mode=insns ;;
    *) inputs+=("$a") ;;
  esac
done

mkdir -p "$WORK/cls"
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
"$JAVAC" -cp "$LIB" -d "$WORK/cls" "$ROOT/tests/oracle/java/JavaInputDump.java"

if [ "$corpus" = 1 ]; then
  SRC="$ROOT/tests/oracle/corpus/java/corpus"
  rm -rf "$WORK/jcorpus"; mkdir -p "$WORK/jcorpus/g" "$WORK/jcorpus/p"
  "$JAVAC" -g -parameters -d "$WORK/jcorpus/g" "$SRC"/*.java
  "$JAVAC" -d "$WORK/jcorpus/p" "$SRC"/*.java
  (cd "$WORK/jcorpus/g" && "$JAR" cf ../corpus_debug.jar corpus)
  (cd "$WORK/jcorpus/p" && "$JAR" cf ../corpus_plain.jar corpus)
  # a jar inside a jar, and a single class file
  mkdir -p "$WORK/jcorpus/nest"
  cp "$WORK/jcorpus/corpus_debug.jar" "$WORK/jcorpus/nest/inner.jar"
  (cd "$WORK/jcorpus/nest" && "$JAR" cf ../corpus_nested.jar inner.jar)
  cp "$WORK/jcorpus/g/corpus/Switches.class" "$WORK/jcorpus/Switches.class"
  inputs+=("$WORK/jcorpus/corpus_debug.jar" "$WORK/jcorpus/corpus_plain.jar" "$WORK/jcorpus/corpus_nested.jar" "$WORK/jcorpus/Switches.class")
fi

fail=0
for f in "${inputs[@]}"; do
  for m in classes insns; do
    if [ "$mode" != both ] && [ "$mode" != "$m" ]; then continue; fi
    "$JAVA" -cp "$WORK/cls:$LIB" JavaInputDump "$m" "$f" 2>/dev/null \
      | grep -E '^(C|CS|CI|CN|CA|CR|F|FS|FA|FR|M|MS|MA|MP[0-9]+|MR|I|T|D|V|E) ' > "$WORK/java.$m.ref.txt" || true
    "$RS" "java-$m" "$f" > "$WORK/java.$m.rs.txt" 2>/dev/null || true
    python3 "$ROOT/tests/oracle/java_diff.py" "$m" "$WORK/java.$m.ref.txt" "$WORK/java.$m.rs.txt" "$m $(basename "$f")" || fail=1
  done
done
exit $fail
