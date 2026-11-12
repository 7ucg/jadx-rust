#!/usr/bin/env bash
# Oracle: compare the complete class source files written by the reference CLI (`jadx -d`) with
# the ones written by jadx-rs, per class and per member, nothing normalised.
#
# usage: source_oracle.sh <input.dex|.apk> [extra decompiler arguments, e.g. --single-class X]
#
# The reference output is cached in $WORK/ref-<key> (key: input name and extra arguments); set
# REUSE_REF=0 to run the reference again.
# Env: JADX_REF (default ../jadx), JDK_BIN (dir with java), WORK (default ./work),
#      N (differing classes to show, default 5), LINES (diff lines per class, default 12)
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JADX_REF="${JADX_REF:-$ROOT/../jadx}"
WORK="${WORK:-$ROOT/work}"
INPUT="$(readlink -f "$1")"; shift
EXTRA=("$@")
KEY="$(basename "$INPUT")$(printf '%s' "${EXTRA[*]:-}" | tr -c 'A-Za-z0-9.' '_')"
REF="$WORK/ref-$KEY"
RS="$WORK/rs-$KEY"
mkdir -p "$WORK"
if [ -n "${JDK_BIN:-}" ]; then export JAVA_HOME="$(dirname "$JDK_BIN")"; fi
cargo build --release -p jadx-cli --manifest-path "$ROOT/Cargo.toml" 2>/dev/null
if [ "${REUSE_REF:-1}" != "1" ] || [ ! -d "$REF/sources" ]; then
  rm -rf "$REF"
  JADX_OPTS="${JADX_OPTS:--Xmx${JAVA_XMX:-5g} -XX:MaxRAMPercentage=40}" "$JADX_REF/jadx-cli/build/install/jadx/bin/jadx" -d "$REF" --no-res --log-level error "${EXTRA[@]}" "$INPUT" >"$WORK/ref-$KEY.log" 2>&1 || true
fi
rm -rf "$RS"
"$ROOT/target/release/jadx-rs" -d "$RS" --no-res --log-level error "${EXTRA[@]}" "$INPUT" >"$WORK/rs-$KEY.log" 2>&1 || true
python3 "$ROOT/tests/oracle/source_diff.py" "$REF/sources" "$RS/sources" "${N:-5}" "${LINES:-12}"
