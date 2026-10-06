#!/usr/bin/env python3
"""Write the plan's `~~~<lang> file=<path>` blocks to disk.

Usage: extract.py <plan.md> <path>...   writes the named blocks
       extract.py <plan.md> --list      lists every block path
Paths are relative to the repo root (the parent of docs/).
"""
import os, re, sys

plan, wanted = sys.argv[1], sys.argv[2:]
root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(plan))))
blocks, cur, buf = {}, None, []
for line in open(plan, encoding="utf-8").read().split("\n"):
    if cur is None:
        m = re.match(r"^~~~\w* file=(\S+)\s*$", line)
        if m:
            cur, buf = m.group(1), []
    elif line == "~~~":
        if cur in blocks:
            sys.exit(f"duplicate block {cur}")
        blocks[cur], cur = "\n".join(buf) + "\n", None
    else:
        buf.append(line)
if cur is not None:
    sys.exit(f"unterminated block {cur}")
if wanted == ["--list"]:
    print("\n".join(blocks))
    sys.exit(0)
for path in wanted:
    if path not in blocks:
        sys.exit(f"no block for {path}")
    dst = os.path.join(root, path)
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    with open(dst, "w", encoding="utf-8") as f:
        f.write(blocks[path])
    print(f"wrote {path}")
