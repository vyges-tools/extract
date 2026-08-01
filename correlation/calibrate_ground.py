#!/usr/bin/env python3
"""Fit the deck's per-layer GROUND capacitance **and** the shielding fraction, across a set.

`calibrate.py` fits ground on one block with no shielding term. That is why the deck tracks
`counter` to 0.997 and runs 1.37x over on a dense one: `counter` is sparse, almost nothing is
shielded, and a fixed fF/um cannot say that field landing on a neighbour is field that did not
land on ground. On a real block the correlation between a net's coupling-to-ground ratio and its
ground error is +0.94.

So fit the model the deck can actually express, with both terms at once:

    ground(net) = sum_L( length_L * c_L )  -  k * Cc(net)

`k` is the deck's `shield_k`, and `Cc` is a net's coupling counted once per net — the same
quantity `engine.rs` subtracts, so what is fitted here is exactly what runs.

Order matters and is not negotiable: `k` multiplies OUR coupling, so this can only be fitted
after `calibrate_coupling.py` has made `Cc` trustworthy. Fitting `k` against a coupling column
that is itself 3x wrong is fitting one error against another.

Isolation uses the real extractor, per layer, as in `calibrate_coupling.py` — ground is linear in
a layer's cap coefficient, so a run with that layer at 1.0 and the rest at 0 yields its column.

    python3 calibrate_ground.py --dir <calset> --deck <deck>.rules --lef <tech>.lef \
                                --extract <binary> [--holdout <block>]
"""

import argparse
import json
import os
import re
import subprocess
from collections import defaultdict

import numpy as np

from decompose import parse_ref, unescape

LAYER_RE = re.compile(r"^(li1|met\d+)$", re.I)
MIN_NETS = 200


def read_deck(path):
    rows, trailer = [], []
    for line in open(path):
        f = line.split("#")[0].split()
        if len(f) >= 4 and LAYER_RE.match(f[0]):
            rows.append((f[0].lower(), [float(x) for x in f[1:5]]))
        elif f and not LAYER_RE.match(f[0]) and not f[0].startswith("shield_k"):
            trailer.append(line.rstrip("\n"))
    return rows, trailer


def write_deck(path, rows, trailer, cap_of, shield_k=None):
    with open(path, "w") as fh:
        for name, v in rows:
            fh.write(f"{name} {v[0]} {cap_of(name, v[1])} {v[2]} {v[3]}\n")
        for t in trailer:
            fh.write(t + "\n")
        if shield_k is not None:
            fh.write(f"shield_k {round(shield_k, 5)}\n")


def run(work, block, extract, deck, lef):
    job = f"{block}.gcal.ext"
    with open(os.path.join(work, job), "w") as fh:
        fh.write(
            f"design: {block}\ndef: {block}.def\nrules: {os.path.basename(deck)}\n"
            f"lef: {os.path.basename(lef)}\ncorner: typical\ntemp: 25\n"
        )
    out = f"{block}.gcal.json"
    # Isolation deliberately zeroes every layer but one, which trips the deck's
    # incomplete-RC guard — correctly, since such a deck would understate a real extraction.
    # This is that guard's intended escape hatch, and the zeros here are the measurement.
    r = subprocess.run(
        [extract, "run", job, "--json", "-o", out, "-q", "--allow-incomplete-rc"],
        cwd=work,
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        raise SystemExit(f"{block}: extract failed\n{r.stderr[-2000:]}")
    j = json.load(open(os.path.join(work, out)))
    g = {unescape(n["name"]): n["ground_cap_ff"] for n in j["per_net"]}
    c = defaultdict(float)
    for x in j["couplings"]:
        c[unescape(x["a"])] += x["cap_ff"]
        c[unescape(x["b"])] += x["cap_ff"]
    return g, c


def pct(v, p):
    v = sorted(v)
    return v[min(len(v) - 1, int(round(p / 100.0 * (len(v) - 1))))] if v else float("nan")


def spread(label, r):
    print(
        f"  {label:<24} n={len(r):>7,}  p10 {pct(r,10):>5.2f}  median {pct(r,50):>5.2f}  "
        f"p90 {pct(r,90):>5.2f}"
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", required=True)
    ap.add_argument("--deck", required=True)
    ap.add_argument("--lef", required=True)
    ap.add_argument("--extract", required=True)
    ap.add_argument("--holdout", default=None)
    a = ap.parse_args()

    work = os.path.abspath(a.dir)
    blocks = sorted(f[:-4] for f in os.listdir(work) if f.endswith(".def"))
    blocks = [b for b in blocks if os.path.exists(os.path.join(work, f"{b}.nom.spef"))]
    rows, trailer = read_deck(a.deck)
    layers = [n for n, _ in rows]
    deck_path = os.path.join(work, os.path.basename(a.deck))
    print(f"{len(blocks)} blocks, layers {layers}")
    if a.holdout:
        print(f"holding out {a.holdout} — scored, never fitted")

    ref = {b: parse_ref(os.path.join(work, f"{b}.nom.spef"))[0] for b in blocks}

    # Our coupling, from the deck AS SHIPPED (already coupling-calibrated) with no shielding —
    # this is the column `shield_k` will multiply, so it must be the calibrated one.
    write_deck(deck_path, rows, trailer, lambda n, c: c, shield_k=0.0)
    cc = {b: run(work, b, a.extract, a.deck, a.lef)[1] for b in blocks}
    print("  captured the calibrated coupling column")

    # Per-layer ground columns: one run per layer at cap = 1.0.
    cols = {}
    for L in layers:
        write_deck(deck_path, rows, trailer, lambda n, _c, L=L: 1.0 if n == L else 0.0, 0.0)
        for b in blocks:
            cols[(b, L)] = run(work, b, a.extract, a.deck, a.lef)[0]
        print(f"  isolated {L}", flush=True)

    def design(bs):
        A, y = [], []
        for b in bs:
            for n, v in ref[b].items():
                r = [cols[(b, L)].get(n, 0.0) for L in layers]
                if v > 1e-4 and any(r):
                    A.append(r + [-cc[b].get(n, 0.0)])
                    y.append(v)
        return np.array(A), np.array(y)

    fit_blocks = [b for b in blocks if b != a.holdout]
    A, y = design(fit_blocks)
    print(f"\nfitting on {len(A):,} nets from {len(fit_blocks)} block(s)")
    try:
        from scipy.optimize import nnls

        coef, _ = nnls(A, y)
    except ImportError:
        coef, *_ = np.linalg.lstsq(A, y, rcond=None)
        coef = np.clip(coef, 0.0, None)
    k = coef[-1]

    seen = {L: int((A[:, i] > 0).sum()) for i, L in enumerate(layers)}
    print(f"\n{'layer':<8}{'deck':>10}{'fitted':>10}{'x':>8}{'nets':>10}")
    for i, L in enumerate(layers):
        d = dict(rows)[L][1]
        print(f"{L:<8}{d:>10.4f}{coef[i]:>10.4f}{coef[i]/d if d else 0:>8.2f}{seen[L]:>10,}")
    # Mind the convention when comparing this to the earlier reference-only estimate of ~0.387:
    # that fit used a net's coupling counted ONCE, this one uses the both-sides sum that
    # `engine.rs` actually subtracts. 0.387/2 = 0.19, so the two agree — they are the same
    # physical fraction expressed against columns that differ by a factor of two.
    print(f"\nshield_k = {k:.4f}   (~{2*k:.2f} of a net's coupling counted once)")

    # Validate. Both models are evaluated on the SAME columns, so the comparison isolates the
    # fit rather than mixing in a re-extraction.
    old = {L: dict(rows)[L][1] for L in layers}
    print("\nPER-NET ground ratio (ours / reference)")
    for b in blocks:
        r_old, r_new = [], []
        for n, v in ref[b].items():
            g = [cols[(b, L)].get(n, 0.0) for L in layers]
            if v <= 1e-4 or not any(g):
                continue
            r_old.append(sum(gi * old[L] for gi, L in zip(g, layers)) / v)
            new = sum(gi * coef[i] for i, gi in enumerate(g)) - k * cc[b].get(n, 0.0)
            r_new.append(max(new, 0.0) / v)
        spread(f"{b} before", r_old)
        spread(f"{b} after" + ("  (HELD OUT)" if b == a.holdout else ""), r_new)

    metals = [L for L in layers if L.startswith("met") and seen[L] >= MIN_NETS]
    stand_in = coef[layers.index(metals[-1])] if metals else None
    carried = [L for L in layers if seen[L] < MIN_NETS]

    def final(name, deck_c):
        if seen[name] >= MIN_NETS:
            return round(coef[layers.index(name)], 5)
        return round(stand_in, 5) if (name.startswith("met") and stand_in) else deck_c

    if carried:
        print("\nCARRIED OVER (too few nets to constrain):")
        for L in carried:
            print(f"  {L:<8}{final(L, dict(rows)[L][1]):>10.5f}   ({seen[L]} nets)")
    write_deck(os.path.join(work, "fitted_ground.rules"), rows, trailer, final, k)
    print(f"\nwrote {os.path.join(work, 'fitted_ground.rules')}")


if __name__ == "__main__":
    main()
