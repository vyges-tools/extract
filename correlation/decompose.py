#!/usr/bin/env python3
"""Split the capacitance gap against a reference SPEF into GROUND and COUPLING, per net.

A single total ratio says "we are N times off" and stops there. Ground and coupling come from
different parts of the model — the per-layer area/fringe coefficients on one side, the lateral
kernel and its neighbour search on the other — so a total is the one number that cannot tell you
which to go fix. This splits them, and buckets by the layers a net actually uses, because the
deck was fitted on a block that never routes above met3.

Two conventions to get right, and both have bitten us:

  * **Units.** A reference SPEF commonly uses `*C_UNIT 1 PF` while ours writes `1 FF`. Read the
    header; never assume.
  * **What `*D_NET` counts.** OpenROAD writes the sum of the entries listed *under* that net, so
    a coupling cap lands in exactly one net's total. Our writer adds a net's coupling from BOTH
    sides. Comparing the two directly overstates our total by roughly the coupling — which is
    how a 1.8x gap was first written down as 2.8x. Here both sides use the same rule: a net's
    coupling is every coupling cap touching it, counted once per net.

Our side is read from `--json` rather than from our own SPEF, because our SPEF folds per-pin
Liberty Cin into the grounded entries and the reference declares `PIN_CAP NONE`. The JSON carries
wire ground and the coupling pair list separately, which is the like-for-like pair.

    python3 decompose.py --ref sign-off.spef --ours ours.json [--def routed.def]
"""

import argparse
import json
import re
from collections import defaultdict


def unescape(name):
    """SPEF escapes characters that could be read as structure — the reference writes
    `u_adapter\\.req_addr_q\\[0\\]` where we write it raw. Same net; joining on the literal
    string silently drops 5.7 % of them, and the ones it drops are exactly the hierarchical
    names, which is not a random sample. Normalise both sides before matching.
    """
    return name.replace("\\", "")


def cap_scale_to_ff(spef_path):
    """`*C_UNIT <n> <unit>` -> multiplier onto femtofarads."""
    per = {"FF": 1.0, "PF": 1e3, "NF": 1e6, "UF": 1e9}
    with open(spef_path) as fh:
        for line in fh:
            f = line.split()
            if f and f[0] == "*C_UNIT":
                return float(f[1]) * per[f[2].upper().rstrip(";")]
            if f and f[0] == "*D_NET":
                break
    raise SystemExit(f"{spef_path}: no *C_UNIT — refusing to guess the units")


def parse_ref(path):
    """-> (ground_ff, coupling_ff) per net name.

    Two passes. The first learns which net each node label belongs to, simply by noting every
    label that appears inside a net's own block. The second needs that, because a coupling entry
    names one node on this net and one on a net defined elsewhere in the file, and both ends have
    to be credited.
    """
    scale = cap_scale_to_ff(path)
    names, blocks = {}, []
    cur = None
    with open(path) as fh:
        for line in fh:
            f = line.split()
            if not f:
                continue
            if re.fullmatch(r"\*\d+", f[0]) and len(f) == 2 and cur is None:
                names[f[0][1:]] = f[1]  # *NAME_MAP entry
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
        """A name-map reference resolves through the map; anything else is literal."""
        if tok.startswith("*"):
            body = tok[1:]
            head, _, tail = body.partition(":")
            if head in names:
                return unescape(names[head]) + (":" + tail if tail else "")
        return unescape(tok)

    owner, ground, coupling = {}, defaultdict(float), defaultdict(float)
    parsed = []
    for b in blocks:
        net = resolve(b["id"])
        sec = None
        own_labels, couples = [], []
        for f in b["lines"]:
            if f[0] in ("*CONN", "*CAP", "*RES"):
                sec = f[0]
                continue
            if sec == "*CONN":
                # `*I <inst>:<pin> <dir>` and `*P <port> <dir>` are both this net's nodes
                if len(f) >= 2:
                    own_labels.append(resolve(f[1]))
            elif sec == "*CAP" and f[0].isdigit():
                if len(f) == 3:
                    ground[net] += float(f[2]) * scale
                    own_labels.append(resolve(f[1]))
                elif len(f) == 4:
                    couples.append((resolve(f[1]), resolve(f[2]), float(f[3]) * scale))
            elif sec == "*RES" and f[0].isdigit() and len(f) == 4:
                own_labels.extend([resolve(f[1]), resolve(f[2])])
        for lab in own_labels:
            owner[lab] = net
        parsed.append((net, couples))
        ground.setdefault(net, 0.0)

    unresolved = 0
    for net, couples in parsed:
        for a, b, v in couples:
            coupling[net] += v
            other = owner.get(b) if owner.get(a) == net else owner.get(a)
            if other is None or other == net:
                unresolved += 1
                continue
            coupling[other] += v
    if unresolved:
        print(f"  note: {unresolved:,} coupling entries whose far end named no known net")
    return dict(ground), dict(coupling)


def parse_ours(path):
    j = json.load(open(path))
    ground = {unescape(n["name"]): n["ground_cap_ff"] for n in j["per_net"]}
    coupling = defaultdict(float)
    for c in j["couplings"]:
        coupling[unescape(c["a"])] += c["cap_ff"]
        coupling[unescape(c["b"])] += c["cap_ff"]
    for n in ground:
        coupling.setdefault(n, 0.0)
    return ground, dict(coupling)


def net_layers(def_path):
    """Net -> the set of routing layers it uses, for bucketing by stack reach."""
    out, cur = {}, None
    layer_tok = re.compile(r"^(li1|met\d+|metal\d+|m\d+)$", re.I)
    with open(def_path) as fh:
        inside = False
        for line in fh:
            s = line.strip()
            if s.startswith("NETS "):
                inside = True
                continue
            if s.startswith("END NETS"):
                break
            if not inside:
                continue
            if s.startswith("- "):
                cur = unescape(s.split()[1])
                out[cur] = set()
            elif cur:
                for i, t in enumerate(s.split()):
                    if t in ("ROUTED", "NEW") or (i == 0 and layer_tok.match(t)):
                        pass
                for t in s.split():
                    if layer_tok.match(t):
                        out[cur].add(t.lower())
    return out


def pct(vals, p):
    if not vals:
        return float("nan")
    v = sorted(vals)
    return v[min(len(v) - 1, int(round(p / 100.0 * (len(v) - 1))))]


def spread(label, ratios):
    if not ratios:
        print(f"  {label:<10} (no nets)")
        return
    print(
        f"  {label:<10} n={len(ratios):>6,}  "
        f"p10 {pct(ratios,10):>6.2f}  p25 {pct(ratios,25):>6.2f}  "
        f"median {pct(ratios,50):>6.2f}  p75 {pct(ratios,75):>6.2f}  p90 {pct(ratios,90):>6.2f}"
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ref", required=True, help="reference SPEF (e.g. the sign-off OpenRCX one)")
    ap.add_argument("--ours", required=True, help="vyges-extract --json output")
    ap.add_argument("--def", dest="def_path", help="routed DEF, to bucket nets by layer reach")
    a = ap.parse_args()

    rg, rc = parse_ref(a.ref)
    og, oc = parse_ours(a.ours)
    common = sorted(set(rg) & set(og))
    print(f"nets: {len(rg):,} reference, {len(og):,} ours, {len(common):,} in both")
    only_ref, only_ours = set(rg) - set(og), set(og) - set(rg)
    if only_ref or only_ours:
        print(f"  {len(only_ref):,} only in the reference, {len(only_ours):,} only in ours")

    print("\nTOTALS over the nets in both (fF)")
    print(f"  {'':<10}{'ours':>14}{'reference':>14}{'ratio':>9}")
    for label, o, r in (
        ("ground", og, rg),
        ("coupling", oc, rc),
    ):
        so, sr = sum(o.get(n, 0.0) for n in common), sum(r.get(n, 0.0) for n in common)
        print(f"  {label:<10}{so:>14,.0f}{sr:>14,.0f}{so/sr if sr else 0:>9.2f}")
    so = sum(og.get(n, 0.0) + oc.get(n, 0.0) for n in common)
    sr = sum(rg.get(n, 0.0) + rc.get(n, 0.0) for n in common)
    print(f"  {'total':<10}{so:>14,.0f}{sr:>14,.0f}{so/sr if sr else 0:>9.2f}")

    # Per-net ratios. Guard the denominator: a reference value of zero is not a 'miss', it is a
    # net the reference had nothing to say about, and dividing by it would invent an outlier.
    FLOOR = 1e-4  # fF
    gr = [og[n] / rg[n] for n in common if rg.get(n, 0.0) > FLOOR]
    cr = [oc.get(n, 0.0) / rc[n] for n in common if rc.get(n, 0.0) > FLOOR]
    print("\nPER-NET ratio (ours / reference)")
    spread("ground", gr)
    spread("coupling", cr)
    zero_ref_c = sum(
        1 for n in common if rc.get(n, 0.0) <= FLOOR and oc.get(n, 0.0) > FLOOR
    )
    if zero_ref_c:
        print(f"  ({zero_ref_c:,} nets where the reference has no coupling and we report some)")

    # One machine-readable line, so a CI gate greps a contract instead of scraping a table
    # whose column layout is free to change.
    print(
        f"\nRATIO ground={sum(og.get(n,0.0) for n in common)/max(sum(rg.get(n,0.0) for n in common),1e-9):.4f}"
        f" coupling={sum(oc.get(n,0.0) for n in common)/max(sum(rc.get(n,0.0) for n in common),1e-9):.4f}"
        f" total={so/max(sr,1e-9):.4f}"
        f" ground_median={pct(gr,50):.4f} coupling_median={pct(cr,50):.4f}"
        f" nets={len(common)}"
    )

    if a.def_path:
        layers = net_layers(a.def_path)
        top = {}
        for n in common:
            ls = layers.get(n, set())
            mets = sorted(int(m[3:]) for m in ls if m.startswith("met") and m[3:].isdigit())
            top[n] = f"up to met{mets[-1]}" if mets else "li1 only"
        print("\nBY STACK REACH — the deck was fitted on a block that stops at met3")
        for bucket in sorted(set(top.values())):
            ns = [n for n in common if top[n] == bucket]
            g = [og[n] / rg[n] for n in ns if rg.get(n, 0.0) > FLOOR]
            c = [oc.get(n, 0.0) / rc[n] for n in ns if rc.get(n, 0.0) > FLOOR]
            print(f"  {bucket:<14} n={len(ns):>6,}")
            spread("  ground", g)
            spread("  coupling", c)


if __name__ == "__main__":
    main()
