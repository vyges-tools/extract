#!/usr/bin/env python3
# klayout_netdump.py — headless KLayout net + geometry dumper for vyges-extract.
#
# Runs KLayout's LayoutToNetlist over a GDS to recover per-net connectivity, then
# emits each net's per-layer metal geometry (layer, width, length, sheet-R) and a
# lumped net capacitance as a neutral line-oriented "net-dump". The Rust front end
# (`vyges-extract klayout2spef`) parses this via the loom KLayout reader and writes
# SPEF + the EM geometry sidecar — so SPEF serialization stays in loom and this
# script stays a thin, dependency-free dumper (only `klayout.db`).
#
# This is a clean-room implementation against the public KLayout Python API.
#
# Layermap format (whitespace, '#' comments):
#   metal <name> <gds_l>/<dt> <rsheet_ohm_sq> <carea_fF_um2> <cedge_fF_um>
#   via   <name> <gds_l>/<dt> <rcut_ohm> <below_metal> <above_metal>
#   text  <gds_l>/<dt>
#
# Net-dump format (consumed by loom::klayout):
#   DESIGN <top>
#   NET  <net> <cap_ff>
#   PIN  <inst> <pin> <dir>
#   SEG  <a> <b> <ohm> <layer> <w_um> <l_um>
#   GCAP <node> <ff>
#
# Usage:
#   python3 klayout_netdump.py --gds F --top CELL --layermap M [--routing-only]
#                              [--out FILE|-]  [--design NAME]

import argparse
import math
import sys


def log(msg):
    print(f"[klayout_netdump] {msg}", file=sys.stderr)


def parse_layermap(path):
    metals, vias, text = [], [], None
    with open(path) as fh:
        for line in fh:
            t = line.split("#", 1)[0].split()
            if not t:
                continue
            kind = t[0]
            try:
                if kind == "metal" and len(t) >= 6:
                    name, ld = t[1], t[2]
                    l, d = ld.split("/")
                    metals.append({
                        "name": name, "layer": int(l), "dtype": int(d),
                        "rsheet": float(t[3]), "carea": float(t[4]), "cedge": float(t[5]),
                    })
                elif kind == "via" and len(t) >= 6:
                    name, ld = t[1], t[2]
                    l, d = ld.split("/")
                    vias.append({
                        "name": name, "layer": int(l), "dtype": int(d),
                        "rcut": float(t[3]), "below": t[4], "above": t[5],
                    })
                elif kind == "text" and len(t) >= 2:
                    l, d = t[1].split("/")
                    text = {"layer": int(l), "dtype": int(d)}
            except ValueError:
                log(f"skip malformed layermap line: {line.strip()}")
    return metals, vias, text


def wire_geom(region, dbu):
    """(area_um2, perimeter_um, width_um, length_um) for a net's shapes on a layer.
    Width = the narrower bbox side (Manhattan wire assumption); length = area/width."""
    area_dbu = region.area()
    if area_dbu == 0:
        return None
    perim_dbu = region.perimeter()
    bbox = region.bbox()
    w_dbu = min(bbox.width(), bbox.height())
    if w_dbu <= 0:
        # degenerate: derive width from area & perimeter (rectangle roots)
        p = perim_dbu / 2.0
        disc = max(p * p - 4.0 * area_dbu, 0.0)
        w_dbu = (p - math.sqrt(disc)) / 2.0 or 1.0
    area = area_dbu * dbu * dbu
    perim = perim_dbu * dbu
    w = w_dbu * dbu
    length = area / w if w > 0 else 0.0
    return area, perim, w, length


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gds", required=True)
    ap.add_argument("--top", default=None)
    ap.add_argument("--layermap", required=True)
    ap.add_argument("--routing-only", action="store_true")
    ap.add_argument("--out", default="-")
    ap.add_argument("--design", default=None)
    args = ap.parse_args()

    import klayout.db as db  # deferred so --help works without the module

    ly = db.Layout()
    ly.read(args.gds)
    dbu = ly.dbu
    top = ly.cell(args.top) if args.top else None
    if top is None:
        top = ly.top_cell()
    design = args.design or (args.top or top.name)
    log(f"gds={args.gds} top={top.name} dbu={dbu}")

    metals, vias, text = parse_layermap(args.layermap)
    if not metals:
        log("no metal layers in layermap — nothing to extract")
        sys.exit(2)

    l2n = db.LayoutToNetlist(db.RecursiveShapeIterator(ly, top, []))
    l2n.dbu = dbu

    regions = {}
    for m in metals:
        r = l2n.make_layer(ly.layer(m["layer"], m["dtype"]), m["name"])
        regions[m["name"]] = r
        l2n.connect(r)  # intra-layer connectivity
    for v in vias:
        r = l2n.make_layer(ly.layer(v["layer"], v["dtype"]), v["name"])
        regions[v["name"]] = r
        if v["below"] in regions and v["above"] in regions:
            l2n.connect(regions[v["below"]], r)
            l2n.connect(r, regions[v["above"]])
    if text is not None:
        tl = l2n.make_text_layer(ly.layer(text["layer"], text["dtype"]), "labels")
        for m in metals:
            l2n.connect(regions[m["name"]], tl)

    l2n.extract_netlist()
    nl = l2n.netlist()

    lines = [f"# vyges-klayout-netdump v1", f"DESIGN {design}"]
    n_nets = 0
    for circuit in nl.each_circuit():
        for net in circuit.each_net():
            name = net.expanded_name()
            if not name:
                continue
            name = name.replace(" ", "_")
            seg_lines, cap_ff = [], 0.0
            for m in metals:
                try:
                    shapes = l2n.shapes_of_net(net, regions[m["name"]], True)
                except Exception:
                    continue
                reg = db.Region(shapes) if not isinstance(shapes, db.Region) else shapes
                g = wire_geom(reg, dbu)
                if g is None:
                    continue
                area, perim, w, length = g
                squares = (length / w) if w > 0 else 0.0
                ohm = m["rsheet"] * squares
                cap_ff += area * m["carea"] + perim * m["cedge"]
                seg_lines.append(
                    f"SEG {name} {name}^{m['name']} {ohm:.6g} {m['name']} {w:.6g} {length:.6g}"
                )
            if not seg_lines:
                continue
            n_nets += 1
            lines.append(f"NET {name} {cap_ff:.6g}")
            # pins: attach one per distinct label on the net (best-effort hookup)
            for pin in net.each_pin():
                pn = pin.name() or "P"
                lines.append(f"PIN {name} {pn} B")
            lines.extend(seg_lines)
            lines.append(f"GCAP {name} {cap_ff:.6g}")

    out = "\n".join(lines) + "\n"
    if args.out == "-":
        sys.stdout.write(out)
    else:
        with open(args.out, "w") as fh:
            fh.write(out)
    log(f"extracted {n_nets} net(s)")


if __name__ == "__main__":
    main()
