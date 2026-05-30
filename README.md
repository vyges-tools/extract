# vyges-extract

Foundry-correlated RC **parasitic extraction**: a routed layout in, a SPEF
parasitic model out.

## Why this exists

On modern nodes the interconnect — not the gate — sets timing and signal
integrity. Static timing analysis only sees that reality if it is handed the
wire resistance and capacitance for every net. That data lives in a **SPEF**
(Standard Parasitic Exchange Format) file, which something has to produce from
the placed-and-routed geometry. `vyges-extract` is that step.

## The problem it solves

Given:

- a **routed design** (`*.def` — the wire geometry), and
- a **per-layer RC rules deck** (`*.rules` — ohms/µm and fF/µm per metal layer),

it emits an **IEEE-1481 SPEF** (`*.spef`): per net, the connected pins, the
grounded capacitance, and the series resistance. It accumulates per-layer
Manhattan wirelength and via counts from the routing and applies the rules.
A net routed on a layer with **no rule is a hard error**, not silent
under-extraction.

This is the same role the OpenROAD `OpenRCX` step plays, exposed as a small,
inspectable, standalone binary behind plain file formats.

## Where it fits in a flow

```text
  *.v   ──[ place + route ]──►  *.def
  *.def ──[ vyges-extract ]──►  *.spef
  *.v + *.spef + *.lib ──[ STA ]──►  timing sign-off
```

The boundary is files in / files out — no in-process API — so it drops into any
flow (LibreLane/OpenROAD or your own) wherever extraction belongs: after detailed
route, before timing/SI sign-off.

## Use it

```sh
cargo build --release            # std-only, no external deps

# 1. write a per-layer rules deck (see examples/counter/sky130.rules)
# 2. write an extraction job pointing at your DEF + rules
# 3. extract:
vyges-extract run  design.ext -o design.spef
vyges-extract check design.ext        # validate the job + inputs
vyges-extract demo                    # print a sample SPEF (no inputs)
```

A job (`*.ext`) is a few `key: value` lines:

```text
design: counter
def:    counter.def        # routed geometry
rules:  sky130.rules       # per-layer R/C
corner: typical
temp:   25
```

A rules deck is a whitespace table:

```text
# layer  res(ohm/um)  cap(fF/um)  [coupling(fF/um)]
met1     0.125        0.078       0.050
via      9.3                                  # default per-via resistance (ohm)
```

A complete, runnable example is in [`examples/counter/`](examples/counter/);
`vyges-extract run examples/counter/counter.ext` prints its SPEF.

## Scope

v0 is a **rule-based lumped** extractor — total grounded C and series R per net.
That is enough to feed STA and to validate the whole `def → spef → timing`
seam. Coupling capacitance, the pi-model / per-pin RC tree, LEF-driven routing
widths, and field-solved correlation against golden patterns build on top of the
same formats; the rules deck already carries a `coupling` column for that.
