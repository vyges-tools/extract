#!/usr/bin/env python3
"""Check our SPEF readers against real files — no golden answer required.

    python3 correlation/spef_conformance.py <file-or-dir> [...]

**Why an oracle-free check.** Golden-file regression (`ours == the .spefok we stored`) only
tells you that today matches yesterday. It cannot tell you that yesterday was right, and both
of the bugs this harness shipped were wrong on day one: coupling caps counted twice, and
coupling entries between node tokens dropped entirely. Every published ratio still read ~1.0.

So these are **invariants**, checkable on any SPEF from any tool:

  1. coupling between nets the file defines is never silently dropped;
  2. the file's listing convention is uniform, and reported rather than assumed;
  3. conservation — the per-net coupling we report sums to exactly twice the file's distinct
     resolvable caps, since each cap loads two nets. Double-counting inflates this, dropped
     entries deflate it, and **both of the shipped bugs violated it**;
  4. our two readers agree with each other;
  5. every `*D_NET` block becomes a net.

Run it over foreign SPEFs — OpenRCX ships golden outputs in `src/rcx/test/*.spefok`, and any
OpenLane run leaves one per block. Measured over those plus our own corpus, six designs list
each cap **twice** and OpenRCX's pattern flow lists each **once**, which is why the readers
dedupe by node pair rather than dividing by two.

Caps naming an aggressor the file never defines are reported, not failed: OpenRCX's pattern
flow does exactly that, because a pattern's neighbours exist in the layout but are not
extracted nets. That is a property of the file, and excluding them is what keeps (3) honest.
"""

import os
import re
import sys
from collections import Counter, defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from decompose import cap_scale_to_ff, parse_ref, unescape  # noqa: E402
from pair_compare import ref_pairs  # noqa: E402


def raw_scan(path):
    """The file as written, independent of either reader — the thing they are checked against.

    -> (distinct caps {node pair: fF}, listings per node pair, unresolvable endpoint count,
        net count, ground entry count)
    """
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

    owner, caps, listings, grounds = {}, {}, Counter(), 0
    pending = {}
    for b in blocks:
        net, sec = resolve(b["id"]), None
        owner[resolve(b["id"])] = net  # the net's own id is a node too
        for f in b["lines"]:
            if f[0] in ("*CONN", "*CAP", "*RES"):
                sec = f[0]
                continue
            if sec == "*CONN" and len(f) >= 2:
                owner[resolve(f[1])] = net
            elif sec == "*CAP" and f[0].isdigit():
                if len(f) == 3:
                    owner[resolve(f[1])] = net
                    grounds += 1
                elif len(f) == 4:
                    key = tuple(sorted((f[1], f[2])))
                    listings[key] += 1
                    caps.setdefault(key, float(f[3]) * scale)
                    pending.setdefault(key, (net, resolve(f[1]), resolve(f[2])))
            elif sec == "*RES" and f[0].isdigit() and len(f) == 4:
                owner[resolve(f[1])] = net
                owner[resolve(f[2])] = net

    # A cap is *resolvable* when both ends name nets this file defines. Some writers reference
    # an aggressor they never write — OpenRCX's pattern flow does, since a pattern's neighbours
    # exist in the layout but are not extracted nets. That is a property of the file, not a
    # reader fault, so it is reported and excluded from conservation rather than failed.
    resolvable, dangling = {}, 0
    for key, (net, a, b) in pending.items():
        other = owner.get(b) if owner.get(a) == net else owner.get(a)
        if other is None or other == net:
            dangling += 1
        else:
            resolvable[key] = caps[key]
    return caps, resolvable, listings, dangling, len(blocks), grounds


CHECKS = []


def check(fn):
    CHECKS.append(fn)
    return fn


@check
def coupling_between_known_nets_is_never_dropped(ctx):
    """A reader that cannot name both ends of a cap drops it and reports a smaller number with
    no error — that is how loom lost 99.5 % of its crosstalk load. Caps whose far end names a
    net the file never defines are counted separately and reported, not failed."""
    caps, resolvable, _, dangling, _, _ = ctx["raw"]
    assert len(resolvable) + dangling == len(caps), "accounting slip in the raw scan"
    _, coupling = ctx["parsed"]
    if resolvable:
        assert sum(coupling.values()) > 0, f"{len(resolvable)} resolvable caps, reader found none"


@check
def the_listing_convention_is_uniform(ctx):
    """SPEF permits a cap in either net's block; writers differ, and one OpenRCX flow differs
    from another. Uniform-and-stated is fine; mixed means dedupe policy needs a second look."""
    _, _, listings, _, _, _ = ctx["raw"]
    kinds = sorted(set(listings.values()))
    ctx["listings"] = kinds
    assert len(kinds) <= 1, f"mixed listing counts in one file: {Counter(listings.values())}"
    assert not kinds or kinds[0] in (1, 2), f"{kinds[0]} listings per cap is not a known convention"


@check
def per_net_coupling_conserves(ctx):
    """Each distinct cap loads exactly two nets, so the per-net totals must sum to 2x the file's
    distinct coupling. Double-counting inflates this; dropped entries deflate it."""
    _, resolvable, _, _, _, _ = ctx["raw"]
    _, coupling = ctx["parsed"]
    want = 2.0 * sum(resolvable.values())
    got = sum(coupling.values())
    assert abs(got - want) <= max(1e-9, 1e-6 * want), f"per-net sum {got:.6g}, file says {want:.6g}"


@check
def the_two_readers_agree(ctx):
    """`decompose` fits the deck, `pair_compare` scores it. Disagreement means one published
    number is wrong and nothing says which."""
    _, coupling = ctx["parsed"]
    pairs, _ = ctx["pairs"]
    from_pairs = defaultdict(float)
    for (a, b), v in pairs.items():
        from_pairs[a] += v
        from_pairs[b] += v
    for net in set(coupling) | set(from_pairs):
        a, b = coupling.get(net, 0.0), from_pairs.get(net, 0.0)
        assert abs(a - b) <= max(1e-9, 1e-6 * max(a, b)), f"{net}: decompose {a:.6g}, pairs {b:.6g}"


@check
def every_net_block_is_read(ctx):
    """Nets parsed must equal `*D_NET` blocks: a name-resolution slip drops nets silently."""
    _, _, _, _, blocks, _ = ctx["raw"]
    ground, _ = ctx["parsed"]
    assert len(ground) == blocks, f"{len(ground)} nets parsed, {blocks} *D_NET blocks"


def run_one(path):
    ctx = {"raw": raw_scan(path), "parsed": parse_ref(path), "pairs": ref_pairs(path)}
    caps, resolvable, listings, dangling, blocks, grounds = ctx["raw"]
    fails = []
    for fn in CHECKS:
        try:
            fn(ctx)
        except AssertionError as e:
            fails.append(f"{fn.__name__}: {e}")
    conv = ctx.get("listings") or [0]
    note = f"  ({dangling} to nets not in the file)" if dangling else ""
    print(
        f"{os.path.basename(path):<32} {blocks:>5} nets {len(caps):>6} caps "
        f"{conv[0]}x listed {grounds:>6} ground  {'FAIL' if fails else 'ok'}{note}"
    )
    for f in fails:
        print(f"    {f}")
    return not fails


def main(argv):
    files = []
    for a in argv:
        if os.path.isdir(a):
            files += [
                os.path.join(a, f)
                for f in sorted(os.listdir(a))
                if f.endswith((".spef", ".spefok"))
            ]
        else:
            files.append(a)
    if not files:
        print(__doc__)
        return 2
    ok = sum(run_one(p) for p in files)
    print(f"\n{ok}/{len(files)} files pass all {len(CHECKS)} invariants")
    return 0 if ok == len(files) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
