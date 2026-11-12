#!/usr/bin/env python3
"""Compare two class level dumps (reference vs jadx-rs `pre-info`).

Usage: pre_info_diff.py <ref.txt> <rs.txt> <name>

Lines are `C|F|M <id> key=value ...` records and `K ...` const storage entries. Records are matched
by kind and id; mismatching keys are counted per kind and key and the first few are printed.
"""

import collections
import re
import sys

KEY_RE = re.compile(r" ([A-Za-z]+)=")


def parse_record(line):
    kind, rest = line.split(" ", 1)
    ident, _, attrs = rest.partition(" ")
    fields = {}
    parts = KEY_RE.split(" " + attrs)
    for i in range(1, len(parts) - 1, 2):
        fields[parts[i]] = parts[i + 1]
    return kind, ident, fields


def load(path):
    records, consts = {}, set()
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if line[:2] not in ("C ", "F ", "M ", "K "):
                continue  # log output of the reference
            if line.startswith("K "):
                consts.add(line)
                continue
            kind, ident, fields = parse_record(line)
            records[(kind, ident)] = fields
    return records, consts


def main():
    ref, rs, name = sys.argv[1], sys.argv[2], sys.argv[3]
    ref_rec, ref_k = load(ref)
    rs_rec, rs_k = load(rs)
    same = 0
    kinds = collections.Counter()
    shown = collections.Counter()
    for key, fields in ref_rec.items():
        other = rs_rec.get(key)
        if other == fields:
            same += 1
            continue
        if other is None:
            kinds[(key[0], "missing in rs")] += 1
            continue
        for k, v in fields.items():
            if other.get(k) != v:
                kinds[(key[0], k)] += 1
                if shown[(key[0], k)] < 3:
                    shown[(key[0], k)] += 1
                    print(
                        f"  {key[0]} {key[1]} {k}\n    ref: {v[:300]}\n    rs : {str(other.get(k))[:300]}"
                    )
    extra = len(set(rs_rec) - set(ref_rec))
    k_missing = sorted(ref_k - rs_k)
    k_extra = sorted(rs_k - ref_k)
    for l in k_missing[:3]:
        print(f"  const only in ref: {l[:200]}")
    for l in k_extra[:3]:
        print(f"  const only in rs : {l[:200]}")
    total = len(ref_rec)
    ok = same == total and not extra and not k_missing and not k_extra
    print(
        f"{'OK  ' if ok else 'DIFF'} {name}: {same}/{total} records identical, {extra} extra in rs, "
        f"const entries ref={len(ref_k)} rs={len(rs_k)} (only ref {len(k_missing)}, only rs {len(k_extra)}); "
        f"mismatches: {kinds.most_common(8)}"
    )
    sys.exit(0 if ok else 1)


main()
