# vyges-extract

Foundry-correlated RC **parasitic extraction**: a routed layout in, a SPEF
parasitic model out.

> **Vyges open EDA tools.** Commercial-grade silicon sign-off capability, built
> on open standards and plain file formats — and meant to be accessible to
> everyone, not only teams who can license a six-figure tool. `vyges-extract`
> opens up parasitic extraction.

## Why this exists

On modern nodes the interconnect — not the gate — sets timing and signal
integrity. Static timing analysis only sees that reality if it is handed the
wire resistance and capacitance for every net. That data lives in a **SPEF**
(Standard Parasitic Exchange Format) file, which something has to produce from
the placed-and-routed geometry. `vyges-extract` is that step.

## How this is solved today

In production, extraction means Synopsys **StarRC** or Cadence **Quantus (QRC)**,
run with foundry-certified tech files and a field solver for the critical nets —
powerful, but gated behind NDA and six-figure licenses. That gate is a big
reason open silicon stalls around 130 nm. The open option, **OpenRCX**
(in OpenROAD), is rule/pattern-based and community-calibrated. The hard part was
never writing an extractor — it is *correlating* one to silicon. `vyges-extract`
starts in that open tier, behind clean file formats, and is built to be
correlated upward without changing how anyone calls it.

**Describe the job, not the script.** Extraction and the tools around it are
typically driven by hand-written **Tcl** control scripts — a recurring source of
silent typos, copy-paste drift, and brittle maintenance. `vyges-extract` takes a
small **declarative job file** (`.ext`: design, DEF, rules, LEF) instead: readable,
diffable, schema-checkable, with no control flow to get wrong. This is a
toolchain-wide property — char, sta-si, and em-ir are configured the same way.

**Validate fast, sign off with your tool.** `vyges-extract` emits **standard SPEF**, so
it drops into any STA / sign-off tool unchanged. Iterate with vyges-extract in the fast
loop, and hand the same design to StarRC or Quantus for final sign-off parasitics if you
prefer — nothing locks you in. It sits *alongside* your flow (correlated to OpenRCX
within ~2% on a real block), the fast checker for the inner loop rather than a
replacement for your golden extractor.

## The problem it solves

Given:

- a **routed design** (`*.def` — the wire geometry),
- a **per-layer RC rules deck** (`*.rules` — ohms/µm and fF/µm per metal layer), and
- *(optional)* a **tech LEF** (`*.lef`) for per-layer routing widths,

it emits an **IEEE-1481 SPEF** (`*.spef`): per net, the connected pins, the
grounded capacitance, the series resistance, and — the hardest, highest-value
term — the **lateral coupling capacitance** to neighbouring nets. Grounded R/C
come from per-layer Manhattan wirelength × the rules; coupling comes from
**geometric adjacency**: same-layer segments of different nets that run parallel
and overlap couple by `coupling_per_um × overlap × (s_ref/gap)`, ignored beyond
`couple_cutoff`. The `gap` is the true **edge-to-edge** spacing when a LEF gives
the routing widths (`gap = centerline − (w_a+w_b)/2`), or the centerline distance
without one. Wires that **cross on different layers** add an *inter-layer* term —
`interlayer[A,B] × footprint-overlap-area` (needs LEF widths). A net routed on a
layer with **no rule is a hard error**, not silent under-extraction.

## Where it fits in a flow

```text
  *.v   ──[ place + route ]──►  *.def
  *.def ──[ vyges-extract ]──►  *.spef
  *.v + *.spef + *.lib ──[ STA ]──►  timing sign-off
```

The boundary is files in / files out — no in-process API — so it drops into any
flow (LibreLane/OpenROAD or your own) wherever extraction belongs: after detailed
route, before timing/SI sign-off.

## When & how to use it in your flow

```text
  netlist ─[OpenROAD: place + route]─► *.def ─[vyges-extract]─► *.spef ─► STA
```

Run it **after detailed route** (once you have a routed `*.def`) and **before
timing sign-off** — static timing analysis can only see wire delay and crosstalk
if it is handed parasitics. Re-run it whenever the routing changes. The SPEF it
emits is exactly what `vyges-sta-si` (or any STA/SI tool) consumes for net
delay. In the open RTL→GDS flow this is the **OpenRCX slot** inside LibreLane,
between the router and the timing/SI step.

## Use it

```sh
# build it yourself (std-only, no deps) -- or grab a binary from GitHub Releases:
cargo build --release            # std-only, no external deps

# 1. write a per-layer rules deck (see examples/counter/sky130.rules)
# 2. write an extraction job pointing at your DEF + rules
# 3. extract:
vyges-extract run  design.ext -o design.spef
vyges-extract run  design.ext --json  # per-net R/C summary instead of SPEF
vyges-extract check design.ext        # validate the job + inputs
vyges-extract demo                    # print a sample SPEF (no inputs)
# common flags: -o FILE · --json · -q/--quiet · -v/--verbose · -h/--help · -V/--version
```

A job (`*.ext`) is a few `key: value` lines:

```text
design: counter
def:    counter.def        # routed geometry
rules:  sky130.rules       # per-layer R/C
lef:    counter.lef        # optional: routing widths -> edge-to-edge coupling gaps
corner: typical
temp:   25
```

A rules deck is a whitespace table:

```text
# layer  res(ohm/um)  cap(fF/um)  [coupling(fF/um)]  [s_ref(um)]
met1     0.125        0.078       0.050              0.14
via      9.3                          # default per-via resistance (ohm)
couple_cutoff 2.0                     # um — ignore lateral coupling beyond this gap
interlayer met1 met2 0.035            # fF/um^2 areal coupling where layers cross
```

A complete, runnable example is in [`examples/counter/`](examples/counter/);
`vyges-extract run examples/counter/counter.ext` prints its SPEF.

## Open core, certified fab plugins

`vyges-extract` is open and contains **no foundry-confidential data**. It runs
out of the box on open PDKs (sky130, gf180) using bundled reference rules.

```text
  vyges-extract — OPEN engine  (Apache-2.0, contains no fab data)
  ────────────────────────────────────────────────────────────────────
    *.def  ─►  def.rs ─► rc.rs ─► spef.rs  ─►  *.spef
                          ▲
                          └─ published plugin contract
                             (.rules: ohm/µm · fF/µm · coupling · per-via Ω)
                                       │
                 loads ONE rules / calibration plugin
                                       │
        ┌──────────────────────────────┴──────────────────────────────┐
        │                                                              │
  OPEN reference plugin                          CERTIFIED per-fab plugins
  (in-repo · no NDA)                             (private · one per fab/node 🔒)
    • sky130A   (.rules)  ✓ M0/M3 validated        • vyges-extract-tsmc28
    • gf180mcu  (.rules)                            • vyges-extract-sec28
                                                    • vyges-extract-micron…
   open data, ships with the tool                silicon-correlated coeffs +
                                                  certified deck — under NDA
```

**sky130A is the starter / reference plugin** — open, no NDA, and already proven
by the M0/M3 runs. Today a "plugin" is just the `.rules` deck you pass on the CLI;
formal per-fab plugin packaging (discovery, signing, repo-per-fab) is the
remaining open item.

Getting *sign-off-grade* output on a **commercial** node takes two things beyond
the tool running: the result must be **correlated to that foundry's silicon**,
and the foundry must **accept the flow under an agreement**. Both live in a
**separate, per-foundry plugin** — never in this repository:

- the open tool defines a published **rules/calibration contract** (the `.rules`
  schema and its calibration extensions);
- a **certified per-foundry plugin** supplies the silicon-correlated coefficients
  and rule sets for a specific node, delivered **under that foundry's NDA**;
- the open engine loads it through the contract and never embeds or references
  any foundry-confidential infrastructure. Each foundry has its own plugin.

So the **engine and the contract are open for everyone**, while the **per-foundry
correlation is gated** to those with the agreement — the same way a commercial
extractor separates its engine from the foundry-delivered techfile, except here
the engine is open. Use `vyges-extract` today on open PDKs and as an
estimation/verification adjunct on any PDK you have; certified sign-off output on
a commercial node comes with that node's plugin.

## Current state (2026-05-31)

**v1** is a **rule-based** extractor with **lateral coupling capacitance** from
segment adjacency: grounded R/C per net plus per-net-pair coupling caps, emitted
in SPEF as a **per-pin RC tree** — a star rooted at the net node, with a trunk
to the driver and a branch to each sink (reducing to a pi for a single sink)
(totals include coupling on both nets). Coupling has both a **lateral** term
(edge-to-edge gap when a tech LEF supplies routing widths, centerline otherwise)
and an **inter-layer** crossover term (areal, over footprint overlap). Runs fully
offline, no external deps, 21 tests green. Enough to feed STA/SI and to validate
the whole `def → spef → timing` seam end to end.

**Correlated against OpenRCX** on a real routed sky130 block (the M0 counter, 45
signal nets, LEF-derived rules from `sky130_fd_sc_hd__nom.tlef`): raw first-principles
rules over-estimate total net cap by ~1.39× (consistent, not random — 45/45 nets
match, no correctness defect); the over-count is dominated by the rule-based lateral
coupling term, and **calibrating it against the golden lands the block total within
~2 % and the per-net cap within ±25 %**. That ±25 % per-net spread is the rule-based
ceiling (real neighbor distances/density a per-µm coefficient can't resolve) — which
is exactly what the M5 field solve closes. See the strategy repo's
`extract-openrcx-correlation.md`.

**M5 field kernel (v1).** An `eps_r <value>` rule turns on a geometry-derived
**2.5-D coupling kernel** — sidewall parallel-plate `Cc = eps_r·eps0·T/gap` from the
LEF metal **thickness** and the real edge-to-edge **gap** — replacing the six
hand-tuned per-layer coupling coefficients with one physical parameter (and fixing
their inverted layer trend: a taller sidewall now correctly couples *more*).
Calibrated against the OpenRCX golden it lands the counter total at 0.99× with a
comparable per-net spread (±~40%) — i.e. it recovers the hand-calibrated accuracy
from first principles with one knob and correct physics, so it generalises. The
effective `eps_r` (~1.45, below the physical ~3.9) shows the bare parallel-plate
over-states coupling. A **fringe-corrected** coupling is available too: with per-layer
`height <um>` rules the kernel applies the Sakurai-style ground-competition fall-off
`exp(-4S/(S+8.01·H))` (coupling falls faster than `1/S` once spacing passes the metal
height, and taller metals couple more) instead of the bare plate.

**Conditional ground-cap shielding** (`shield_k <0..1>`): a net's coupling is field
that would otherwise be grounded fringe, so the grounded cap is reduced by
`shield_k · Cc_net` (charge conservation) — making it neighbour-dependent. At
`shield_k = 0.5` it lets the effective `eps_r` rise from 1.45 toward the physical
~3.9 (2.3) while conserving charge, at comparable accuracy.

**What the M5 study established (the honest ceiling).** Three independent levers —
fringe-corrected coupling, a global fringe cut, and conditional shielding — were each
measured against the OpenRCX golden, and **none tighten the ±40% per-net spread**. So
that spread is the **rule-based / analytic ceiling**: the calibrated model nails the
block total (~2 %) and is portable, but per-net accuracy below ±40 % depends on
detailed local geometry (exact multi-neighbour configurations, density) that no
global coefficient captures. Closing it requires **true per-net field solving**
against the actual layout — the research-grade endpoint commercial QRC/StarRC occupy.

The road to sign-off grade is therefore a genuine **2.5-D field/pattern-matched
solver** per net (not more global coefficients), plus a geometry-aware
moment-weighted RC tree. Same file formats and CLI; same `run` command, no license.
