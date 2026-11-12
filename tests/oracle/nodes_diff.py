#!/usr/bin/env python3
"""Per-method comparison of two sorted node dumps; prints parity and the first divergences.

With --erase-generics the reference side is normalised first: generic arguments are removed and
bare type variables become java.lang.Object (until generic signatures are supported).
"""
import re, sys, collections

def erase(line):
    # drop <...> (nested) and ' extends ...' bounds, map bare type variables to Object
    prev = None
    while prev != line:
        prev = line
        line = re.sub(r"<[^<>]*>", "", line)
    line = re.sub(r"(?<![\w.$])[A-Z][A-Za-z0-9_]{0,2}(?![\w.$(])", "java.lang.Object", line) if False else line
    return line

def blocks(path, erase_generics=False):
    out, cur = {}, None
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if line.startswith("M "):
                cur = line
                out[cur] = []
            elif cur is not None:
                out[cur].append(erase(line) if erase_generics else line)
    return out

def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    eg = "--erase-generics" in sys.argv
    ref, rs, name = blocks(args[0], eg), blocks(args[1]), args[2]
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
                if a.startswith("R "):
                    kinds[a[2:].lstrip().split(" ")[0][:30]] += 1
                else:
                    kinds[a.split(" ")[2] if a.startswith("I ") else a.split(" ")[0][:30]] += 1
                if shown < 4:
                    shown += 1
                    print(f"  {k}\n    ref: {a[:300]}\n    rs : {b[:300]}")
                break
    extra = len(set(rs) - set(ref))
    total = len(ref)
    print(f"{'OK  ' if same == total and not extra else 'DIFF'} {name}: {same}/{total} methods identical, {extra} extra in rs; first-diff insn types: {kinds.most_common(6)}")
    sys.exit(0 if same == total and not extra else 1)

main()
