#!/usr/bin/env python3
"""Per-method comparison of two java input dumps (instruction or class dumps).

Usage: java_diff.py <insns|classes> <ref.txt> <rs.txt> [name] [shown]

`insns`: a block starts with an `M <method>` header; blocks are compared by header and printed
as parity (identical methods / methods in the reference). `classes`: a block starts with a
`C <source> <type>` line and covers the class members.
Exit status 0 when everything is identical.
"""

import collections
import sys


def blocks(path, start):
    out = collections.OrderedDict()
    cur = None
    dup = collections.Counter()
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if line.startswith(start):
                key = line if start == "M " else line.split("\t")[1]
                dup[key] += 1
                if dup[key] > 1:
                    key = f"{key}#{dup[key]}"
                cur = key
                out[cur] = [line]
            elif cur is not None:
                out[cur].append(line)
    return out


def kind(line):
    parts = line.split(" ")
    if line.startswith("I "):
        return "insn " + parts[2]
    return parts[0]


def main():
    mode, ref_path, rs_path = sys.argv[1:4]
    name = sys.argv[4] if len(sys.argv) > 4 else ""
    shown_max = int(sys.argv[5]) if len(sys.argv) > 5 else 4
    start = "M " if mode == "insns" else "C "
    ref, rs = blocks(ref_path, start), blocks(rs_path, start)
    same = 0
    kinds = collections.Counter()
    shown = 0
    for key, v in ref.items():
        w = rs.get(key)
        if w == v:
            same += 1
            continue
        if w is None:
            kinds["missing in rs"] += 1
            continue
        for a, b in zip(v + ["<EOF>"], w + ["<EOF>"]):
            if a != b:
                kinds[kind(a)] += 1
                if shown < shown_max:
                    shown += 1
                    print(f"  {key[:160]}\n    ref: {a[:300]}\n    rs : {b[:300]}")
                break
    extra = sum(1 for k in rs if k not in ref)
    total = len(ref)
    unit = "methods" if mode == "insns" else "classes"
    pct = 100.0 * same / total if total else 100.0
    print(
        f"{'OK  ' if same == total and not extra else 'DIFF'}  {same}/{total} {unit} identical ({pct:.2f}%), extra in rs: {extra}  {name}"
    )
    if kinds:
        print(
            "    first differing line kinds: "
            + ", ".join(f"{k}={n}" for k, n in kinds.most_common(8))
        )
    return 0 if same == total and not extra else 1


if __name__ == "__main__":
    sys.exit(main())
