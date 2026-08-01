#!/usr/bin/env python3
"""What per-layer ground-cap coefficients does a reference SPEF actually imply?

Our grounded capacitance is `sum(segment length x layer fF/um)` and nothing else — no per-node
or per-via term. So a per-net ratio that varies with net size cannot come from an additive
offset; it comes from the **layer mix** changing with size, which means the error is in the
per-layer coefficients themselves. This solves for them directly.

Per net, the routed length on each layer is the design matrix; the reference's grounded
capacitance for that net is the target; least squares gives the fF/um the reference behaves as if
it used. Comparing that to the deck says which layer is wrong and by how much — where a single
total ratio says only "something is".

Two things this deliberately does NOT do:

  * **It does not fit against our own numbers.** The target is the reference's ground column, so
    the answer does not depend on our model being right about anything except length.
  * **It does not claim the residual is noise.** A per-layer model cannot represent width or
    spacing dependence, and the reference resolves both from tables. The residual spread it
    leaves is reported, because that is the part re-fitting coefficients can never fix.

    python3 fit_ground.py --ref sign-off.spef --def routed.def --deck <deck>.rules
"""

import argparse
import re
from collections import defaultdict

import numpy as np

from decompose import parse_ref, unescape

LAYER = re.compile(r"^(li1|met\d+|metal\d+|m\d+)$", re.I)


def net_layer_lengths(def_path):
    """Per net, the routed length in microns on each layer.

    Mirrors the reader in `vyges-tools/loom`, including the two things that reader had to learn
    the hard way: `( * y )` repeats the previous coordinate, and a `RECT ( dx dy dx dy )` patch
    is an OFFSET rectangle whose body must be skipped — read as a coordinate it draws a wire to
    near the origin, which here would silently inflate a layer's fitted length.
    """
    text = open(def_path).read()
    start = text.find("\nNETS ")
    end = text.find("\nEND NETS")
    toks = re.findall(r"\(|\)|[^\s()]+", text[start:end])

    out = defaultdict(lambda: defaultdict(float))
    i, cur, layer, prev, routing = 0, None, None, None, False
    while i < len(toks):
        t = toks[i]
        if t == "-" and i + 1 < len(toks):
            cur, layer, prev, routing = unescape(toks[i + 1]), None, None, False
            out[cur]  # materialise, so a net with no routing still appears
            i += 2
            continue
        if t == ";":
            cur, routing = None, False
            i += 1
            continue
        if t == "+" and toks[i + 1 : i + 2] and toks[i + 1] in ("ROUTED", "FIXED", "COVER"):
            routing, layer, prev = True, toks[i + 2], None
            i += 3
            continue
        if t == "NEW":
            layer, prev = toks[i + 1], None
            i += 2
            continue
        if t == "RECT":
            i += 1
            if toks[i : i + 1] == ["("]:
                while i < len(toks) and toks[i] != ")":
                    i += 1
                i += 1
            continue
        if t == "(":
            j, inner = i + 1, []
            while j < len(toks) and toks[j] != ")":
                inner.append(toks[j])
                j += 1
            if routing and cur and len(inner) >= 2:
                px, py = prev if prev else (0, 0)
                x = px if inner[0] == "*" else int(inner[0])
                y = py if inner[1] == "*" else int(inner[1])
                if prev is not None and layer and LAYER.match(layer):
                    d = abs(x - prev[0]) + abs(y - prev[1])
                    if d:
                        out[cur][layer.lower()] += d
                prev = (x, y)
            i = j + 1
            continue
        i += 1
    return out


def read_deck(path):
    caps = {}
    for line in open(path):
        line = line.split("#")[0].split()
        if len(line) >= 3 and LAYER.match(line[0]):
            caps[line[0].lower()] = float(line[2])
    return caps


def pct(v, p):
    v = sorted(v)
    return v[min(len(v) - 1, int(round(p / 100.0 * (len(v) - 1))))] if v else float("nan")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ref", required=True)
    ap.add_argument("--def", dest="def_path", required=True)
    ap.add_argument("--deck", required=True)
    ap.add_argument("--dbu", type=float, default=1000.0, help="DEF units per micron")
    a = ap.parse_args()

    rg, _ = parse_ref(a.ref)
    lens = net_layer_lengths(a.def_path)
    deck = read_deck(a.deck)

    nets = sorted(set(rg) & set(lens))
    layers = sorted({l for n in nets for l in lens[n]})
    print(f"{len(nets):,} nets with both a reference ground cap and routing, layers {layers}")

    A = np.array([[lens[n].get(l, 0.0) / a.dbu for l in layers] for n in nets])
    b = np.array([rg[n] for n in nets])
    fit, *_ = np.linalg.lstsq(A, b, rcond=None)

    print("\nPER-LAYER GROUND CAP (fF/um)")
    print(f"  {'layer':<8}{'routed um':>14}{'deck':>10}{'implied':>10}{'deck/implied':>14}")
    for j, l in enumerate(layers):
        tot = A[:, j].sum()
        d = deck.get(l, float("nan"))
        print(f"  {l:<8}{tot:>14,.0f}{d:>10.4f}{fit[j]:>10.4f}{d / fit[j]:>14.2f}")

    # How much of the ground error does a per-layer model explain at all? Re-predict with the
    # fitted coefficients and look at what spread is left — that residual is the width/spacing
    # dependence a per-um deck structurally cannot carry, and re-fitting will not remove it.
    pred = A @ fit
    ours = A @ np.array([deck.get(l, 0.0) for l in layers])
    keep = b > 1e-4
    r_now = (ours[keep] / b[keep]).tolist()
    r_fit = (pred[keep] / b[keep]).tolist()
    print("\nPER-NET ground ratio vs the reference")
    for label, r in (("deck today", r_now), ("best per-layer fit", r_fit)):
        print(
            f"  {label:<20} median {pct(r,50):>5.2f}   "
            f"p10 {pct(r,10):>5.2f}  p90 {pct(r,90):>5.2f}   "
            f"total {sum(ours if label.startswith('deck') else pred):,.0f} fF"
        )
    print(f"  {'reference':<20}{'':>13}{'':>22}   total {b.sum():,.0f} fF")


if __name__ == "__main__":
    main()
