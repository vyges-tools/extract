#!/usr/bin/env python3
"""Self-test for the SPEF readers the correlation harness fits against.

Run it directly — no pytest, no dependencies:

    python3 correlation/test_readers.py

**Why this file exists.** For a month the deck's coupling coefficient was ~2x physical and the
cause was here, not in the engine: `parse_ref` credited every `*CAP` listing, and OpenRCX writes
each coupling cap in BOTH nets' blocks. Every published ratio still read ~1.0, because the deck was
fitted to the doubled reference and doubled with it. Nothing failed, because nothing was testing
the reader. These cases pin the conventions that mistake turned on.
"""

import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from decompose import parse_ref  # noqa: E402
from pair_compare import ref_pairs  # noqa: E402

HEADER = """\
*SPEF "IEEE 1481-1999"
*DESIGN "blk"
*DATE "x"
*DIVIDER /
*DELIMITER :
*BUS_DELIMITER []
*T_UNIT 1 NS
*C_UNIT {unit}
*R_UNIT 1 OHM
*L_UNIT 1 HENRY

*NAME_MAP
*1 neta
*2 netb
*3 u1
*4 u2
"""

# A coupling cap of 2 fF between neta and netb, written the way OpenRCX writes it: once in
# each net's block, at full value, with node-token endpoints.
BOTH_BLOCKS = (
    HEADER.format(unit="1 FF")
    + """
*D_NET *1 10
*CONN
*I *3:A I
*CAP
1 *3:A 8
2 *3:A *4:Y 2
*END

*D_NET *2 7
*CONN
*I *4:Y O
*CAP
1 *4:Y 5
2 *3:A *4:Y 2
*END
"""
)

# The same design, written by a tool that lists each coupling cap ONCE. Both conventions are
# legal; the reader must give the same answer for both.
ONE_BLOCK = (
    HEADER.format(unit="1 FF")
    + """
*D_NET *1 10
*CONN
*I *3:A I
*CAP
1 *3:A 8
2 *3:A *4:Y 2
*END

*D_NET *2 7
*CONN
*I *4:Y O
*CAP
1 *4:Y 5
*END
"""
)

# Same again in picofarads — the unit OpenRCX actually emits.
IN_PICOFARADS = BOTH_BLOCKS.replace("*C_UNIT 1 FF", "*C_UNIT 1 PF").replace(
    "2 *3:A *4:Y 2", "2 *3:A *4:Y 0.002"
)

# A coupling entry may name the NET NODE itself (`*1`) rather than an internal node. Found by
# `spef_conformance.py` on OpenRCX's own pattern-flow output, where every endpoint is of this
# form — an owner map built only from *CONN/*RES/ground entries resolves none of them.
NET_NODE_ENDPOINTS = (
    HEADER.format(unit="1 FF")
    + """
*D_NET *1 10
*CAP
1 *1 8
2 *1 *2 2
*END

*D_NET *2 7
*CAP
1 *2 5
2 *1 *2 2
*END
"""
)

# A two-node cap between two nodes of the SAME net is an intra-net cap, not crosstalk.
INTRA_NET = (
    HEADER.format(unit="1 FF")
    + """
*D_NET *1 10
*CONN
*I *3:A I
*RES
1 *1 *3:A 100
*CAP
1 *1 8
2 *1 *3:A 2
*END
"""
)

CASES = []


def case(fn):
    CASES.append(fn)
    return fn


def read(text):
    """-> (ground, coupling, pairs) from both readers over the same text."""
    fd, path = tempfile.mkstemp(suffix=".spef")
    try:
        with os.fdopen(fd, "w") as fh:
            fh.write(text)
        ground, coupling = parse_ref(path)
        pairs, _ = ref_pairs(path)
    finally:
        os.unlink(path)
    return ground, coupling, pairs


@case
def a_cap_in_both_blocks_is_counted_once():
    """THE regression. Crediting both listings doubles every coupling value in the corpus."""
    ground, coupling, pairs = read(BOTH_BLOCKS)
    assert coupling["neta"] == 2.0, f"neta coupling {coupling['neta']}, want 2.0 (not 4.0)"
    assert coupling["netb"] == 2.0, f"netb coupling {coupling['netb']}, want 2.0 (not 4.0)"
    assert pairs[("neta", "netb")] == 2.0, f"pair {pairs[('neta','netb')]}, want 2.0"
    # ground is per-node and listed once, so it was never affected — pin that it stays so.
    assert ground["neta"] == 8.0 and ground["netb"] == 5.0


@case
def both_writer_conventions_give_the_same_answer():
    """Deduping by node pair must not penalise a writer that already lists each cap once."""
    _, c_both, p_both = read(BOTH_BLOCKS)
    _, c_one, p_one = read(ONE_BLOCK)
    assert c_both == c_one, f"{c_both} != {c_one}"
    assert dict(p_both) == dict(p_one)


@case
def node_token_endpoints_resolve_to_their_nets():
    """Endpoints are instance pins, not net names; the far end lives in another net's block."""
    _, coupling, pairs = read(BOTH_BLOCKS)
    assert set(coupling) == {"neta", "netb"}, coupling
    assert list(pairs) == [("neta", "netb")], pairs


@case
def a_cap_naming_the_net_node_resolves():
    """Endpoints are not always instance pins — OpenRCX's pattern flow names the net node."""
    _, coupling, pairs = read(NET_NODE_ENDPOINTS)
    assert coupling.get("neta") == 2.0, f"neta {coupling.get('neta')}, want 2.0"
    assert coupling.get("netb") == 2.0, f"netb {coupling.get('netb')}, want 2.0"
    assert pairs.get(("neta", "netb")) == 2.0, pairs


@case
def picofarad_units_scale_to_femtofarads():
    """OpenRCX writes *C_UNIT 1 PF. A missed scale is a clean 1000x, and would look like ours."""
    _, coupling, _ = read(IN_PICOFARADS)
    assert abs(coupling["neta"] - 2.0) < 1e-9, coupling["neta"]


@case
def an_intra_net_cap_is_not_coupling():
    _, coupling, pairs = read(INTRA_NET)
    assert coupling.get("neta", 0.0) == 0.0, coupling
    assert not pairs, pairs


@case
def the_two_readers_agree():
    """`decompose` fits the deck and `pair_compare` scores it. If they ever disagree about what
    the reference says, one of the two published numbers is wrong."""
    for text in (BOTH_BLOCKS, ONE_BLOCK, IN_PICOFARADS, INTRA_NET, NET_NODE_ENDPOINTS):
        _, coupling, pairs = read(text)
        from_pairs = {}
        for (a, b), v in pairs.items():
            from_pairs[a] = from_pairs.get(a, 0.0) + v
            from_pairs[b] = from_pairs.get(b, 0.0) + v
        for net, v in from_pairs.items():
            assert abs(coupling.get(net, 0.0) - v) < 1e-9, f"{net}: {coupling.get(net)} vs {v}"


def main():
    failed = 0
    for fn in CASES:
        try:
            fn()
            print(f"  ok    {fn.__name__}")
        except AssertionError as e:
            failed += 1
            print(f"  FAIL  {fn.__name__}: {e}")
    print(f"\n{len(CASES) - failed}/{len(CASES)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
