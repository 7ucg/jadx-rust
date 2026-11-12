#!/usr/bin/env python3
"""Compare two directories of decoded resources (reference vs jadx-rs) byte for byte.

usage: res_diff.py REF_DIR RS_DIR LABEL [MAX_SHOWN]

Prints one summary line "LABEL: identical/total files identical" and, for the first MAX_SHOWN
(default 5) differing files, the first differing line. Exit status 1 if any file differs or is
missing on one side.
"""

import os
import sys


def collect(root):
    files = {}
    for base, _dirs, names in os.walk(root):
        for n in names:
            p = os.path.join(base, n)
            files[os.path.relpath(p, root)] = p
    return files


def first_diff(a, b):
    la = a.split(b"\n")
    lb = b.split(b"\n")
    for i in range(max(len(la), len(lb))):
        x = la[i] if i < len(la) else b"<missing>"
        y = lb[i] if i < len(lb) else b"<missing>"
        if x != y:
            return i + 1, x.decode("utf-8", "replace"), y.decode("utf-8", "replace")
    return 0, "", ""


def main():
    ref_dir, rs_dir, label = sys.argv[1:4]
    shown_max = int(sys.argv[4]) if len(sys.argv) > 4 else 5
    ref = collect(ref_dir)
    rs = collect(rs_dir)
    names = sorted(set(ref) | set(rs))
    same = 0
    shown = 0
    for n in names:
        if n not in ref or n not in rs:
            if shown < shown_max:
                print(f"  {n}: only in {'reference' if n in ref else 'jadx-rs'}")
                shown += 1
            continue
        a = open(ref[n], "rb").read()
        b = open(rs[n], "rb").read()
        if a == b:
            same += 1
            continue
        if shown < shown_max:
            line, x, y = first_diff(a, b)
            print(f"  {n}: line {line}")
            print(f"    ref: {x[:160]}")
            print(f"    rs : {y[:160]}")
            shown += 1
    print(f"{label}: {same}/{len(names)} files identical")
    sys.exit(0 if same == len(names) else 1)


if __name__ == "__main__":
    main()
