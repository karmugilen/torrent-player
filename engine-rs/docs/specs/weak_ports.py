#!/usr/bin/env python3
"""List wt-engine tests whose body is under 40% of the matching engine-go test body."""
import glob
import re
import sys


def bodies(files, pat):
    out = {}
    for f in files:
        src = open(f).read()
        for m in re.finditer(pat, src):
            i, depth = m.end(), 1
            while depth and i < len(src):
                depth += {"{": 1, "}": -1}.get(src[i], 0)
                i += 1
            body = src[m.end():i - 1]
            n = len([l for l in body.splitlines() if l.strip() and not l.strip().startswith("//")])
            out[m.group("name")] = (f, n, m.group(0))
    return out


root = sys.argv[1] if len(sys.argv) > 1 else "."
go = bodies(glob.glob(f"{root}/engine-go/*_test.go"), r"func (?P<name>Test\w+)\(t \*testing\.T\) \{")
rs = bodies(glob.glob(f"{root}/engine-rs/crates/wt-engine/tests/engine_*.rs"),
            r"(?P<attrs>(?:#\[[^\]]*\]\s*)+)(?:async\s+)?fn (?P<name>test_\w+)\s*\([^)]*\)[^{]*\{")
weak = []
for g, (_, gn, _) in go.items():
    key = re.sub(r"(?<!^)(?=[A-Z])", "_", g[4:]).lower().replace("_", "")
    cand = [k for k in rs if k[5:].replace("_", "") == key]
    if not cand:
        continue
    rf, rn, hdr = rs[cand[0]]
    if gn >= 12 and rn < gn * 0.4:
        weak.append((rn / gn, cand[0], rf.split("/")[-1], gn, rn, "ignored" if "ignore" in hdr else "RUNS"))
weak.sort()
print(len(weak), "tests whose Rust body is under 40% of the Go body (Go >= 12 lines)")
for w in weak:
    print("%-75s %-32s go=%3d rs=%3d %s" % (w[1], w[2], w[3], w[4], w[5]))
