#!/usr/bin/env python3
"""Is the crossover column distinguishable from the lateral one, or just collinear with it?

NNLS zeroing a term with 33,000 constraining nets has two very different readings: the term
contributes nothing, or it contributes something the fit cannot separate from another column.
Only the second is a limitation of the data rather than a fact about the physics, and they are
told apart by looking at the columns, not the coefficients.
"""
import json
import os
import subprocess
import sys
from collections import defaultdict

import numpy as np

from decompose import unescape

BLOCK = sys.argv[1] if len(sys.argv) > 1 else "CF_UART_APB"
EXT = os.path.expanduser("~/rcxcorr/extract-src/target/release/vyges-extract")
LEF = "sky130_fd_sc_hd__nom.tlef"


def col(tag, deck_body):
    open("collin.rules", "w").write(deck_body)
    open("collin.ext", "w").write(
        f"design: {BLOCK}\ndef: {BLOCK}.def\nrules: collin.rules\nlef: {LEF}\n"
        "corner: typical\ntemp: 25\n"
    )
    r = subprocess.run(
        [EXT, "run", "collin.ext", "--json", "-o", "collin.json", "-q", "--allow-incomplete-rc"],
        capture_output=True,
        text=True,
    )
    if r.returncode:
        raise SystemExit(f"{tag}: {r.stderr[-800:]}")
    c = defaultdict(float)
    for x in json.load(open("collin.json"))["couplings"]:
        c[unescape(x["a"])] += x["cap_ff"]
        c[unescape(x["b"])] += x["cap_ff"]
    return c


STACK = """li1 12.8 0.06 {li1} 0.17
met1 0.125 0.1259 {met1} 0.14
met2 0.125 0.1211 {met2} 0.14
met3 0.047 0.1371 {met3} 0.3
met4 0.047 0.1371 {met4} 0.3
met5 0.0285 0.1371 {met5} 1.6
via 9.3
couple_cutoff 2.0
"""
zero = dict.fromkeys(["li1", "met1", "met2", "met3", "met4", "met5"], 0.0)

lat = col("lateral met1", STACK.format(**{**zero, "met1": 1.0}))
xov = col("crossover met1/met2", STACK.format(**zero) + "interlayer met1 met2 1.0\n")

nets = sorted(set(lat) | set(xov))
a = np.array([lat.get(n, 0.0) for n in nets])
b = np.array([xov.get(n, 0.0) for n in nets])
both = (a > 0) & (b > 0)
print(f"{BLOCK}: {len(nets):,} nets   lateral>0 {int((a>0).sum()):,}   crossover>0 {int((b>0).sum()):,}")
print(f"  correlation over nets where both are non-zero (n={int(both.sum()):,}): "
      f"{np.corrcoef(a[both], b[both])[0,1]:+.4f}")
print(f"  column totals: lateral {a.sum():,.1f}   crossover {b.sum():,.1f}  "
      f"(per unit coefficient, so magnitudes are not comparable — the SHAPE is)")
