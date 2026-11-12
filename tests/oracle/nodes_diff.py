#!/usr/bin/env python3
"""Per-method comparison of two sorted node dumps; prints parity and the first divergences."""
import sys, collections

def blocks(path):
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
    ref, rs, name = blocks(sys.argv[1]), blocks(sys.argv[2]), sys.argv[3]
    same = 0
    kinds = collections.Counter()
    shown = 0
    for k, v in ref.items():
        w = rs.get(k)
        if w == v:
            same += 1
            continue
        if w is None:
            kinds["missing in rs"] += 1
            continue
        for a, b in zip(v + ["<EOF>"], w + ["<EOF>"]):
            if a != b:
                kinds[a.split(" ")[2] if a.startswith("I ") else a[:30]] += 1
                if shown < 4:
                    shown += 1
                    print(f"  {k}\n    ref: {a[:300]}\n    rs : {b[:300]}")
                break
    extra = len(set(rs) - set(ref))
    total = len(ref)
    print(f"{'OK  ' if same == total and not extra else 'DIFF'} {name}: {same}/{total} methods identical, {extra} extra in rs; first-diff insn types: {kinds.most_common(6)}")
    sys.exit(0 if same == total and not extra else 1)

main()
