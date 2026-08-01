#!/usr/bin/env python3
"""Is the ground-capacitance error neighbour-dependent — i.e. is it shielding?

Field that lands on a neighbour is field that did not land on ground. A deck with one fixed fF/um
per layer cannot express that, so it must over-state ground wherever the layout is dense — and a
per-layer refit will then quietly absorb a *density* error into a *length* coefficient, which
looks right on the block it was fitted to and wrong everywhere else. That is worth ruling in or
out before touching a single coefficient.

Three things, on the reference's own numbers so the answer does not depend on our model:

  A. fit `ref_ground ~ sum(len_L * c_L)` — all a per-um deck can say;
  B. add a shielding term, `- k * ref_coupling`, and see whether the residual collapses;
  C. correlate a net's coupling-to-ground ratio against how far our deck over-states its ground.

Run from a directory holding the SPEF, the DEF and the deck. Note that a stray `bisect.py` beside
it will shadow the stdlib module numpy imports — that is not hypothetical, it cost a debug cycle.
"""
import numpy as np
from decompose import parse_ref
from fit_ground import net_layer_lengths, read_deck

DECK = read_deck("sky130A.vyges-extract.rules")

rg, rc = parse_ref("fft_ctrl_tlul.nom.spef")
lens = net_layer_lengths("fft_ctrl_tlul.def")
nets = sorted(set(rg) & set(lens))
layers = sorted({l for n in nets for l in lens[n]})

L = np.array([[lens[n].get(l, 0.0) / 1000.0 for l in layers] for n in nets])
g = np.array([rg[n] for n in nets])
# the reference's own coupling, counted once per net (its *D_NET convention)
c = np.array([rc.get(n, 0.0) / 2.0 for n in nets])


def report(label, A, cols):
    fit, *_ = np.linalg.lstsq(A, g, rcond=None)
    pred = A @ fit
    keep = g > 1e-4
    r = np.sort(pred[keep] / g[keep])
    p = lambda q: r[min(len(r) - 1, int(round(q / 100.0 * (len(r) - 1))))]
    print(f"\n{label}")
    for name, v in zip(cols, fit):
        print(f"    {name:<14}{v:>10.4f}")
    print(
        f"    per-net ratio  median {p(50):.3f}   p10 {p(10):.3f}  p90 {p(90):.3f}"
        f"   (p90-p10 = {p(90)-p(10):.3f})"
    )
    print(f"    residual RMS   {np.sqrt(((pred - g) ** 2).mean()):.4f} fF")
    return fit


report("A. per-layer only (what the deck can express)", L, layers)
report(
    "B. per-layer + a shielding term  ground = sum(len*c) - k*coupling",
    np.hstack([L, -c[:, None]]),
    layers + ["k (shield)"],
)

# How strong is the raw association? If dense nets are the over-estimated ones, the ratio of
# coupling to ground should track the error.
keep = (g > 1e-4) & (c > 1e-4)
ratio_err = np.array([sum(lens[n].get(l, 0.0) / 1000.0 * d for l, d in DECK.items()) for n in nets])[
    keep
] / g[keep]
dens = c[keep] / g[keep]
print(
    f"\nC. correlation(coupling/ground , deck_ground/ref_ground) = "
    f"{np.corrcoef(dens, ratio_err)[0,1]:+.3f}   over {keep.sum():,} nets"
)
