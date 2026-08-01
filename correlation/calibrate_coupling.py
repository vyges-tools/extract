#!/usr/bin/env python3
"""Fit the deck's per-layer COUPLING coefficients against reference SPEFs, across a SET of blocks.

`calibrate.py` fits the per-layer *ground* capacitance on one block. Coupling was never fitted to
anything, which is how a wrong routing width could inflate it two-and-a-half fold with no test
noticing. This is the missing half.

Method, mirroring `calibrate.py`'s isolation trick so there is no re-implementation to drift:

  1. **Isolate.** Coupling is linear in a layer's coefficient, so run the real extractor once per
     layer with that layer's coupling set to 1.0 and every other layer's set to 0. The per-net
     result is that layer's geometric contribution — the design matrix column.
  2. **Regress** the reference's per-net coupling on those columns (non-negative least squares).
     Because the columns were built at coefficient 1.0, the solution *is* the coefficient.
  3. **Validate** by re-running with the fitted deck and reporting the per-net spread.

Both sides count a net's coupling once per net (i.e. a cap between A and B is charged to both),
so the convention matches on either side of the comparison — the mismatch that first turned a
1.8x gap into a reported 2.8x one.

Fitting across blocks is the point, not a detail: one block cannot separate a coefficient from
that block's own density, which is exactly how the previous deck came to be right on `counter`
and wrong everywhere else.

    python3 calibrate_coupling.py --dir <calset> --deck <deck>.rules --lef <tech>.lef \
                                  --extract <binary> [--holdout <block>]
"""

import argparse
import json
import os
import re
import subprocess
import sys
from collections import defaultdict

import numpy as np

from decompose import parse_ref, unescape

LAYER_RE = re.compile(r"^(li1|met\d+)$", re.I)


def read_deck(path):
    """-> [(layer, [res, cap, coupling, s_ref])], plus the non-layer trailer lines.

    Existing `interlayer` rows are dropped from the trailer: this script fits them, so
    carrying the old ones through would emit each twice.
    """
    rows, trailer = [], []
    for line in open(path):
        f = line.split("#")[0].split()
        if len(f) >= 4 and LAYER_RE.match(f[0]):
            rows.append((f[0].lower(), [float(x) for x in f[1:5]]))
        elif f and not LAYER_RE.match(f[0]) and f[0].lower() != "interlayer":
            trailer.append(line.rstrip("\n"))
    return rows, trailer


def write_deck(path, rows, trailer, coupling_of, inter=()):
    """`inter` is an iterable of (layerA, layerB, fF/um2) crossover rows."""
    with open(path, "w") as fh:
        for name, v in rows:
            c = coupling_of(name, v[2])
            fh.write(f"{name} {v[0]} {v[1]} {c} {v[3]}\n")
        for t in trailer:
            fh.write(t + "\n")
        for a, b, c in inter:
            fh.write(f"interlayer {a} {b} {c}\n")


def our_coupling(work, block, extract, deck, lef):
    """Run the extractor and return {net: coupling fF}, a net's coupling counted once per net."""
    job = os.path.join(work, f"{block}.cal.ext")
    with open(job, "w") as fh:
        fh.write(
            f"design: {block}\ndef: {block}.def\nrules: {os.path.basename(deck)}\n"
            f"lef: {os.path.basename(lef)}\ncorner: typical\ntemp: 25\n"
        )
    out = os.path.join(work, f"{block}.cal.json")
    r = subprocess.run(
        [extract, "run", os.path.basename(job), "--json", "-o", os.path.basename(out), "-q"],
        cwd=work,
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        raise SystemExit(f"{block}: extract failed\n{r.stderr[-2000:]}")
    j = json.load(open(out))
    c = defaultdict(float)
    for x in j["couplings"]:
        c[unescape(x["a"])] += x["cap_ff"]
        c[unescape(x["b"])] += x["cap_ff"]
    return c


def pct(v, p):
    v = sorted(v)
    return v[min(len(v) - 1, int(round(p / 100.0 * (len(v) - 1))))] if v else float("nan")


def spread(label, r):
    print(
        f"  {label:<22} n={len(r):>7,}  p10 {pct(r,10):>5.2f}  median {pct(r,50):>5.2f}  "
        f"p90 {pct(r,90):>5.2f}"
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", required=True, help="directory of <block>.def + <block>.nom.spef")
    ap.add_argument("--deck", required=True)
    ap.add_argument("--lef", required=True)
    ap.add_argument("--extract", required=True)
    ap.add_argument("--holdout", default=None, help="fit without this block, then score it")
    ap.add_argument(
        "--no-interlayer",
        action="store_true",
        help="lateral only — reproduces the earlier fit, for comparison",
    )
    a = ap.parse_args()

    work = os.path.abspath(a.dir)
    blocks = sorted(f[:-4] for f in os.listdir(work) if f.endswith(".def"))
    blocks = [b for b in blocks if os.path.exists(os.path.join(work, f"{b}.nom.spef"))]
    rows, trailer = read_deck(a.deck)
    layers = [n for n, _ in rows]
    print(f"{len(blocks)} blocks, layers {layers}")
    if a.holdout:
        print(f"holding out {a.holdout} — it is scored but never fitted")

    ref = {b: parse_ref(os.path.join(work, f"{b}.nom.spef"))[1] for b in blocks}

    # The terms to fit. Lateral is one coefficient per layer; crossover is one per ADJACENT
    # layer pair — non-adjacent pairs are screened by the metal between them, and including
    # them mostly buys collinearity with the adjacent pair.
    #
    # Fitting both together is the point. They compete for the same reference coupling, so
    # fitting lateral alone does not leave crossover unmodelled — it silently loads crossover
    # onto the lateral coefficient, which is exactly the state this is meant to end.
    terms = [("lat", L) for L in layers]
    if not a.no_interlayer:
        terms += [("x", (layers[i], layers[i + 1])) for i in range(len(layers) - 1)]
    label = {("lat", L): L for L in layers}
    label.update({("x", p): f"{p[0]}/{p[1]}" for k, p in terms if k == "x"})

    # ---- 1. isolate: one run per (block, term), that term at 1.0 and every other at 0 ----
    deck_path = os.path.join(work, os.path.basename(a.deck))
    cols = {}
    for t in terms:
        kind, what = t
        write_deck(
            deck_path,
            rows,
            trailer,
            (lambda n, _c, L=what: 1.0 if n == L else 0.0) if kind == "lat" else (lambda n, _c: 0.0),
            inter=[(what[0], what[1], 1.0)] if kind == "x" else (),
        )
        for b in blocks:
            cols[(b, t)] = our_coupling(work, b, a.extract, a.deck, a.lef)
        print(f"  isolated {label[t]}", flush=True)

    # ---- 2. regress the reference's per-net coupling on those columns ----
    def rows_for(bs):
        A, y, tag = [], [], []
        for b in bs:
            for n, v in ref[b].items():
                r = [cols[(b, t)].get(n, 0.0) for t in terms]
                if v > 1e-4 and any(r):
                    A.append(r)
                    y.append(v)
                    tag.append(b)
        return np.array(A), np.array(y), tag

    fit_blocks = [b for b in blocks if b != a.holdout]
    A, y, _ = rows_for(fit_blocks)
    print(f"\nfitting on {len(A):,} nets from {len(fit_blocks)} block(s)")
    try:
        from scipy.optimize import nnls

        coef, _ = nnls(A, y)
    except ImportError:
        coef, *_ = np.linalg.lstsq(A, y, rcond=None)
        coef = np.clip(coef, 0.0, None)

    print(f"\n{'term':<12}{'fitted':>12}{'nets seen':>12}")
    for i, t in enumerate(terms):
        seen_i = int((A[:, i] > 0).sum())
        print(f"{label[t]:<12}{coef[i]:>12.5f}{seen_i:>12,}")
        if seen_i < 200:
            print(f"             ^ only {seen_i} nets constrain this — treat it as unfitted")

    # ---- 3. validate, per block, before and after ----
    print("\nPER-NET coupling ratio (ours / reference)")
    old = {("lat", L): dict(rows)[L][2] for L in layers}
    for b in blocks:
        r_old, r_new = [], []
        for n, v in ref[b].items():
            g = [cols[(b, t)].get(n, 0.0) for t in terms]
            if v <= 1e-4 or not any(g):
                continue
            r_old.append(sum(gi * old.get(t, 0.0) for gi, t in zip(g, terms)) / v)
            r_new.append(sum(gi * coef[i] for i, gi in enumerate(g)) / v)
        mark = "  (HELD OUT)" if b == a.holdout else ""
        spread(f"{b} before", r_old)
        spread(f"{b} after {mark}", r_new)

    # A term no net constrains gets a least-squares answer of 0.0, and writing that would
    # silently switch it off. Carry a placeholder instead, and say which.
    MIN_NETS = 200
    fit_of = {t: coef[i] for i, t in enumerate(terms)}
    seen_of = {t: int((A[:, i] > 0).sum()) for i, t in enumerate(terms)}

    lat_ok = [L for L in layers if L.startswith("met") and seen_of[("lat", L)] >= MIN_NETS]
    stand_in = fit_of[("lat", lat_ok[-1])] if lat_ok else None

    def lateral(name, deck_c):
        t = ("lat", name)
        if seen_of[t] >= MIN_NETS:
            return round(fit_of[t], 5)
        # Unconstrained: the topmost well-fitted metal stands in for a metal layer; anything
        # else keeps what the deck already said, since we have learnt nothing about it.
        return round(stand_in, 5) if (name.startswith("met") and stand_in) else deck_c

    crossover = [
        (p[0], p[1], round(fit_of[t], 6))
        for t in terms
        if t[0] == "x" and seen_of[t] >= MIN_NETS and fit_of[t] > 0
        for p in [t[1]]
    ]

    carried = [label[t] for t in terms if seen_of[t] < MIN_NETS]
    if carried:
        print(f"\nNOT FITTED (under {MIN_NETS} constraining nets): {', '.join(carried)}")
        print("  lateral layers carry a placeholder; crossover pairs are simply omitted.")
    write_deck(
        os.path.join(work, "fitted.rules"), rows, trailer, lateral, inter=crossover
    )
    print(f"\nwrote {os.path.join(work, 'fitted.rules')}")


if __name__ == "__main__":
    main()
