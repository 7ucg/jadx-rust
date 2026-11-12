#!/usr/bin/env python3
"""Normalise a node dump: sort method blocks by method id so class iteration order is irrelevant."""
import sys

def main():
    blocks, cur = {}, None
    with open(sys.argv[1], encoding="utf-8", errors="replace") as f:
        for line in f:
            if line.startswith("M "):
                cur = line.rstrip("\n")
                blocks.setdefault(cur, [])
            elif cur is not None and (line.startswith(("H ", "T ", "L ", "B ", "  I ", "I ", "  P ", "V ", "E ", "W ", "R "))):
                blocks[cur].append(line.rstrip("\n"))
    out = sys.stdout
    for k in sorted(blocks):
        out.write(k + "\n")
        for l in blocks[k]:
            out.write(l + "\n")

main()
