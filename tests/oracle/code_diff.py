#!/usr/bin/env python3
"""Compare two code dumps (see sort_code.py) line by line.

Reports the share of identical code strings overall and per instruction type, plus the first
divergences. Statement lines (S) and wrapped instruction lines (W) are counted separately.
Options: --show=N prints N divergences (default 12), --type=NAME restricts the printed
divergences to one instruction type.
"""

import collections
import sys


def load(path):
    out, cur = {}, None
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if line.startswith("M "):
                cur = line
                out[cur] = []
            elif cur is not None:
                out[cur].append(line)
    return out


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    show, only = 12, None
    for a in sys.argv[1:]:
        if a.startswith("--show="):
            show = int(a[7:])
        if a.startswith("--type="):
            only = a[7:]
    ref, rs, name = load(args[0]), load(args[1]), args[2]
    total = same = 0
    per = collections.defaultdict(lambda: [0, 0])
    kinds = collections.defaultdict(lambda: [0, 0])
    shown = 0
    methods_same = 0
    for k, v in ref.items():
        w = rs.get(k)
        if w is None:
            total += len(v)
            for l in v:
                p = l.split(" ", 3)
                per[p[2]][0] += 1
                kinds[p[0]][0] += 1
            continue
        methods_same += w == v
        wmap = {}
        for l in w:
            p = l.split(" ", 3)
            wmap[(p[0], p[1])] = l
        for l in v:
            p = l.split(" ", 3)
            total += 1
            per[p[2]][0] += 1
            kinds[p[0]][0] += 1
            if wmap.get((p[0], p[1])) == l:
                same += 1
                per[p[2]][1] += 1
                kinds[p[0]][1] += 1
            elif shown < show and (only is None or p[2] == only):
                shown += 1
                print(
                    f"  {k}\n    ref: {l[:400]}\n    rs : {wmap.get((p[0], p[1]), '<missing>')[:400]}"
                )
    extra = len(set(rs) - set(ref))
    pct = 100.0 * same / total if total else 100.0
    ok = same == total and not extra
    print(
        f"{'OK  ' if ok else 'DIFF'} {name}: {same}/{total} code strings identical ({pct:.2f}%), "
        f"{methods_same}/{len(ref)} methods identical, {extra} extra methods in rs"
    )
    print(
        "  by kind: "
        + ", ".join(f"{k} {a[1]}/{a[0]}" for k, a in sorted(kinds.items()))
    )
    worst = sorted(per.items(), key=lambda kv: kv[1][1] - kv[1][0])[:10]
    print(
        "  worst types: "
        + ", ".join(f"{k} {a[1]}/{a[0]}" for k, a in worst if a[1] != a[0])
    )
    sys.exit(0 if ok else 1)


main()
