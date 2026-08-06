#!/usr/bin/env python3
"""Compare coupling NET PAIR by NET PAIR against a reference SPEF.

A total ratio near 1.0 can mean the model is right, or it can mean two errors cancelling. Only
comparing the pair sets tells them apart:

  * pairs both find, and how the per-pair values compare — is the coefficient right?
  * pairs only we find — are we coupling nets that are not really neighbours, or just reporting
    below the reference's threshold?
  * pairs only the reference finds — adjacency we are blind to.

The reference's node labels are resolved to nets by noting every label that appears inside a
net's own block; a coupling entry names one node here and one on a net defined elsewhere, so
both ends have to be credited. Names are unescaped on both sides — the reference writes
`u_adapter\\.q\\[0\\]` where we write it raw, and joining on the literal string silently drops
the hierarchical names, which is not a random sample.

    python3 pair_compare.py --ref sign-off.spef --ours ours.json
"""

import argparse
import json
import re
from collections import defaultdict

from decompose import cap_scale_to_ff, unescape


def ref_pairs(path):
    scale = cap_scale_to_ff(path)
    names, blocks, cur = {}, [], None
    for line in open(path):
        f = line.split()
        if not f:
            continue
        if re.fullmatch(r"\*\d+", f[0]) and len(f) == 2 and cur is None:
            names[f[0][1:]] = f[1]
            continue
        if f[0] == "*D_NET":
            cur = {"id": f[1], "lines": []}
            blocks.append(cur)
            continue
        if f[0] == "*END":
            cur = None
            continue
        if cur is not None:
            cur["lines"].append(f)

    def resolve(tok):
        if tok.startswith("*"):
            head, _, tail = tok[1:].partition(":")
            if head in names:
                return unescape(names[head]) + (":" + tail if tail else "")
        return unescape(tok)

    owner, pending = {}, []
    for b in blocks:
        net, sec = resolve(b["id"]), None
        for f in b["lines"]:
            if f[0] in ("*CONN", "*CAP", "*RES"):
                sec = f[0]
                continue
            if sec == "*CONN" and len(f) >= 2:
                owner[resolve(f[1])] = net
            elif sec == "*CAP" and f[0].isdigit():
                if len(f) == 3:
                    owner[resolve(f[1])] = net
                elif len(f) == 4:
                    pending.append((net, resolve(f[1]), resolve(f[2]), float(f[3]) * scale))
            elif sec == "*RES" and f[0].isdigit() and len(f) == 4:
                owner[resolve(f[1])] = net
                owner[resolve(f[2])] = net

    # ONE physical cap, TWO listings — OpenRCX writes each coupling cap in BOTH nets'
    # blocks with the same value, so crediting every listing doubles it. Count each node
    # pair once. (See the same fix in `decompose.parse_ref`.)
    by_pair = defaultdict(list)
    for net, a, b, v in pending:
        by_pair[tuple(sorted((a, b)))].append((net, a, b, v))

    pairs, unresolved = defaultdict(float), 0
    for entries in by_pair.values():
        for net, a, b, v in entries:
            other = owner.get(b) if owner.get(a) == net else owner.get(a)
            if other is None or other == net:
                continue
            pairs[tuple(sorted((net, other)))] += v
            break
        else:
            unresolved += 1
    return pairs, unresolved


def our_pairs(path):
    p = defaultdict(float)
    for c in json.load(open(path))["couplings"]:
        p[tuple(sorted((unescape(c["a"]), unescape(c["b"]))))] += c["cap_ff"]
    return p


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ref", required=True)
    ap.add_argument("--ours", required=True)
    # OpenRCX lumps coupling below a threshold into ground rather than reporting it, so pairs
    # under it are expected to be missing from the reference and are not a defect.
    ap.add_argument("--threshold", type=float, default=0.1, help="reference's coupling threshold, fF")
    a = ap.parse_args()

    ref, unresolved = ref_pairs(a.ref)
    ours = our_pairs(a.ours)
    if unresolved:
        print(f"note: {unresolved:,} reference coupling entries whose far end named no known net")

    both = set(ref) & set(ours)
    only_o, only_r = set(ours) - set(ref), set(ref) - set(ours)
    print(f"\n{'':<24}{'pairs':>12}{'fF (ours)':>14}{'fF (ref)':>14}")
    print(f"{'found by both':<24}{len(both):>12,}{sum(ours[p] for p in both):>14,.0f}"
          f"{sum(ref[p] for p in both):>14,.0f}")
    print(f"{'only ours':<24}{len(only_o):>12,}{sum(ours[p] for p in only_o):>14,.0f}{'—':>14}")
    print(f"{'only the reference':<24}{len(only_r):>12,}{'—':>14}{sum(ref[p] for p in only_r):>14,.0f}")

    r = sorted(ours[p] / ref[p] for p in both if ref[p] > 1e-6)
    q = lambda x: r[min(len(r) - 1, int(round(x / 100.0 * (len(r) - 1))))] if r else float("nan")
    print(f"\nper-pair ratio on the {len(r):,} shared pairs:")
    print(f"  p10 {q(10):.2f}  p25 {q(25):.2f}  median {q(50):.2f}  p75 {q(75):.2f}  p90 {q(90):.2f}")

    below = [p for p in only_o if ours[p] < a.threshold]
    above = [p for p in only_o if ours[p] >= a.threshold]
    print(f"\npairs only we report, against the reference's {a.threshold} fF threshold:")
    print(f"  below  {len(below):>8,} pairs  {sum(ours[p] for p in below):>9,.0f} fF"
          f"   — expected: the reference lumps these into ground")
    print(f"  at/above {len(above):>6,} pairs  {sum(ours[p] for p in above):>9,.0f} fF"
          f"   — adjacency we claim and the reference does not")
    if above:
        print("\n  largest of those:")
        for p in sorted(above, key=lambda k: -ours[k])[:5]:
            print(f"    {ours[p]:>8.2f} fF   {p[0]}  <->  {p[1]}")


if __name__ == "__main__":
    main()
