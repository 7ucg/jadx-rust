#!/usr/bin/env python3
"""Compare the class source files of two decompiler outputs (nothing is normalised).

usage: source_diff.py <ref-sources-dir> <rs-sources-dir> [N=5] [LINES=12]

Prints the number of identical classes, a member level comparison (the members of a class are
the runs of lines separated by blank lines) and the first differing lines of the first N
differing classes.
"""

import difflib
import os
import sys


def class_files(root):
    out = {}
    for d, _, files in os.walk(root):
        for f in files:
            if f.endswith(".java"):
                p = os.path.join(d, f)
                out[os.path.relpath(p, root)] = p
    return out


def read(path):
    with open(path, encoding="utf-8", errors="replace", newline="") as f:
        return f.read()


def chunks(text):
    """Members of the class text: runs of lines separated by blank lines."""
    out, cur = [], []
    for line in text.split("\n"):
        if line.strip() == "":
            if cur:
                out.append("\n".join(cur))
                cur = []
        else:
            cur.append(line)
    if cur:
        out.append("\n".join(cur))
    return out


def main():
    ref_dir, rs_dir = sys.argv[1], sys.argv[2]
    max_show = int(sys.argv[3]) if len(sys.argv) > 3 else 5
    lines_show = int(sys.argv[4]) if len(sys.argv) > 4 else 12
    ref, rs = class_files(ref_dir), class_files(rs_dir)
    total = len(ref)
    same = 0
    missing = []
    differing = []
    chunk_total = chunk_same = 0
    for rel, rp in sorted(ref.items()):
        rt = read(rp)
        sp = rs.get(rel)
        if sp is None:
            missing.append(rel)
            chunk_total += len(chunks(rt))
            continue
        st = read(sp)
        rc = chunks(rt)
        sc = chunks(st)
        chunk_total += len(rc)
        pool = {}
        for c in sc:
            pool[c] = pool.get(c, 0) + 1
        for c in rc:
            if pool.get(c, 0) > 0:
                pool[c] -= 1
                chunk_same += 1
        if rt == st:
            same += 1
        else:
            differing.append(rel)
    extra = len(set(rs) - set(ref))
    pct = 100.0 * same / total if total else 0.0
    cpct = 100.0 * chunk_same / chunk_total if chunk_total else 0.0
    print(
        f"classes identical: {same} / {total} ({pct:.1f}%), missing: {len(missing)}, extra: {extra}"
    )
    print(f"members identical: {chunk_same} / {chunk_total} ({cpct:.1f}%)")
    for rel in missing[:max_show]:
        print(f"MISSING {rel}")
    for rel in differing[:max_show]:
        a = read(ref[rel]).split("\n")
        b = read(rs[rel]).split("\n")
        print(f"--- DIFF {rel}")
        shown = 0
        for line in difflib.unified_diff(a, b, "ref", "rs", n=1, lineterm=""):
            if line.startswith(("---", "+++")):
                continue
            print(line)
            shown += 1
            if shown >= lines_show:
                print("...")
                break
    sys.exit(0 if same == total and not missing else 1)


if __name__ == "__main__":
    main()
