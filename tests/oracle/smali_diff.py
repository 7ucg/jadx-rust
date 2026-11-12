#!/usr/bin/env python3
"""Compare two smali dumps (reference vs jadx-rs) class by class.

Both files use `#---end-class---` as class separator. Prints parity (identical classes and
identical methods) and the first differing lines of the first N differing classes.
"""
import sys, re, collections

def classes(path):
    """Stream classes (separated by a `#---end-class---` line) without loading the whole file."""
    buf = []
    with open(path, encoding="utf-8", errors="replace", newline="\n") as f:
        for line in f:
            if line == "#---end-class---\n":
                text = "".join(buf)
                buf = []
                yield text[:-1] if text.endswith("\n") else text
            else:
                buf.append(line)
    if buf:
        yield "".join(buf)

def methods(cls):
    out = {}
    cur = None
    for line in cls.split("\n"):
        if line.startswith(".method "):
            cur = line
            out[cur] = [line]
        elif cur is not None:
            out[cur].append(line)
            if line == ".end method":
                cur = None
    return out

def main():
    ref, rs = classes(sys.argv[1]), classes(sys.argv[2])
    show = int(sys.argv[3]) if len(sys.argv) > 3 else 3
    n = 0
    same = 0
    msame = mtotal = 0
    shown = 0
    kinds = collections.Counter()
    from itertools import zip_longest
    for i, (a, b) in enumerate(zip_longest(ref, rs)):
        if a is None or b is None:
            print(f"class count differs at #{i}")
            break
        n += 1
        ma, mb = methods(a), methods(b)
        for k, v in ma.items():
            mtotal += 1
            if mb.get(k) == v:
                msame += 1
        if a == b:
            same += 1
            continue
        al, bl = a.split("\n"), b.split("\n")
        for j in range(max(len(al), len(bl))):
            x = al[j] if j < len(al) else "<EOF>"
            y = bl[j] if j < len(bl) else "<EOF>"
            if x != y:
                kinds[re.sub(r"[0-9a-fx_]+", "N", x.strip())[:40]] += 1
                if shown < show:
                    shown += 1
                    print(f"--- class #{i} line {j}")
                    print("ref:", al[max(0, j-3):j+3])
                    print("rs :", bl[max(0, j-3):j+3])
                break
    print(f"classes identical: {same}/{n} ({100.0*same/max(n,1):.2f}%)")
    print(f"methods identical: {msame}/{mtotal} ({100.0*msame/max(mtotal,1):.2f}%)")
    print("first-diff kinds:", kinds.most_common(8))

main()
