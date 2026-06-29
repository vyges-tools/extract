# vyges-extract

Foundry-correlated RC **parasitic extraction**: a routed layout in, a SPEF
parasitic model out.

> **Vyges open EDA tools.** Commercial-grade silicon sign-off capability, built
> on open standards and plain file formats — and meant to be accessible to
> everyone, not only teams who can license a six-figure tool. `vyges-extract`
> opens up parasitic extraction.

**Docs:** [docs.vyges.com](https://docs.vyges.com) — this engine's chapter, the
[cross-engine integration guide](https://docs.vyges.com/engines/integration.html) (how the four
Vyges engines work together and where each plugs into an OpenROAD / LibreLane flow), and the
job-file formats. **Integrating at the binary level and need help?** → <https://vyges.com/contact>.

## Why this exists

On modern nodes the interconnect — not the gate — sets timing and signal
integrity. Static timing analysis only sees that reality if it is handed the
wire resistance and capacitance for every net. That data lives in a **SPEF**
(Standard Parasitic Exchange Format) file, which something has to produce from
the placed-and-routed geometry. `vyges-extract` is that step.

## How this is solved today

In production, extraction means a **commercial sign-off extractor**,
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
loop, and hand the same design to your commercial extractor for final sign-off parasitics if you
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
come from per-layer Manhattan wirelength × the rules; resistance is
**width-dependent** when the deck gives a sheet resistance (`R = rsheet × len /
width`, the width from a per-segment non-default rule if one applies, else the LEF),
otherwise the width-blind `res × len`. Coupling comes
from **geometric adjacency**: same-layer segments of different nets that run parallel
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
rsheet   met1 0.125                   # sheet resistance (ohm/sq) -> width-dependent R = rsheet*len/width
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
    *.def  ─►  def.rs ─► rc.rs + tree.rs ─► spef.rs  ─►  *.spef
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
remaining open item. The **calibrated** sky130A deck lives at
[`pdk/sky130A/sky130A.vyges-extract.rules`](pdk/sky130A/sky130A.vyges-extract.rules):
its per-layer caps are fit to the OpenRCX nom golden on a routed block, so total
capacitance tracks OpenRCX to **0.997** (per-net mean ~1.0, σ 0.13) — vs ~0.60 for
an uncalibrated deck. Method + harness in
[`correlation/openrcx-counter.md`](correlation/openrcx-counter.md).

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

## Domain coverage — digital *and* analog / mixed-signal

The RC model is **geometry × rules** (`rc.rs`, `coupling.rs`): per-net wirelength
per layer × ohm/µm and fF/µm, plus adjacency coupling. Nothing in that math is
std-cell-, clock-, or Liberty-specific — there is **no Liberty dependency** — so it
is **domain-agnostic**. What couples extraction to a domain is only the *input
format*: it consumes routed signal nets as DEF `NETS` (per-segment layer + Manhattan
endpoints + pins) via the shared `vyges_loom` DEF reader. Any routed layout in that
form extracts identically.

- **Analog routed layouts supplied as DEF extract today, unchanged.**
  [`examples/bias_gen/`](examples/bias_gen/) is a small analog bias generator with a
  long thin **bias line** (met1, resistance matters), a **high-impedance sensitive
  node** carried up the stack (met1→via→met2), and a **wide supply tap** (met3) —
  exercising multiple layers and vias. `tests/analog.rs` runs extraction and asserts
  sane RC: R and C > 0 per net, R scales with wirelength, the via adds its rule
  resistance, and the sensitive node's coupling to the bias line is captured (while
  the supply tap, beyond `couple_cutoff`, is correctly left uncoupled).

  ```sh
  vyges-extract run  examples/bias_gen/bias_gen.ext           # -> SPEF
  vyges-extract run  examples/bias_gen/bias_gen.ext --json    # -> per-net R/C
  ```

- **GDS-only analog** (no routed DEF) has two on-ramps. The simplest is to **emit a
  routed DEF** from your router and use the validated DEF path above. For raw GDS,
  an optional **`gds → DefNet` connectivity-tracing front-end** ([`src/gds.rs`](src/gds.rs))
  traces connected wire geometry into the same `DefNet` view the RC core consumes:
  it flattens the GDS, classifies rectangles by a small layer map
  (`GDS layer/datatype → routing name | via`), unions touching same-layer wires
  (cross-layer joins are **contact-gated** — only where a via rect overlaps both),
  reduces each net to centerline segments + a via count, and names nets from TEXT
  labels. It sits strictly **above** `rc.rs` (it produces `DefNet`s; the RC math is
  untouched) and is exercised by `tests/gds_extract.rs` (GDS → trace → RC →
  coupling). Its honest bounds: axis-aligned rectangles (polygon bends bbox'd),
  via = overlap (no enclosure DRC), and no instance/pin hookup (GDS carries none, so
  the SPEF uses the lumped form) — see the module header for the full list. For a
  digital block the routed-DEF path is and remains the primary input.

Scope here is **physical RC extraction**. Per-net field-solve accuracy (the ±40 %
analytic ceiling below) and analog *functional/timing* sign-off remain external/
research-grade work.

## Current state (2026-05-31)

**v1** is a **rule-based** extractor with **lateral coupling capacitance** from
segment adjacency: grounded R/C per net plus per-net-pair coupling caps, emitted
in SPEF as a **distributed RC tree built from the routing geometry** — routing
vertices become nodes, wire segments become resistors with end-split caps, and via
stacks become resistors between the per-layer sub-nodes, so the SPEF carries genuine
internal wire-junction nodes (not a star). Node caps and resistances are scaled back
to the calibrated per-net totals, so the distributed model adds topology without
re-correlating magnitudes; a net with no usable geometry falls back to a lumped star
(totals include coupling on both nets). Coupling has both a **lateral** term
(edge-to-edge gap when a tech LEF supplies routing widths, centerline otherwise)
and an **inter-layer** crossover term (areal, over footprint overlap). Resistance is
**width-dependent** when the deck supplies a per-layer sheet resistance (`R = rsheet ×
len / width`, the width taken from a per-segment non-default rule (`NONDEFAULTRULE` /
`TAPERRULE`) when one applies, else the LEF), versus the width-blind `res × len`.
Coupling extraction is spatially indexed (a uniform grid), so cost scales with routed area
rather than net-count squared. Runs fully offline, no external deps, 45 tests green.
Enough to feed STA/SI and to validate the whole `def → spef → timing` seam end to end.

**Correlated against OpenRCX** on a real routed sky130 block (the M0 counter, 45
signal nets, LEF-derived rules from `sky130_fd_sc_hd__nom.tlef`): raw first-principles
rules over-estimate total net cap by ~1.39× (consistent, not random — 45/45 nets
match, no correctness defect); the over-count is dominated by the rule-based lateral
coupling term, and **calibrating it against the golden lands the block total within
~2 % and the per-net cap within ±25 %**. That ±25 % per-net spread is the rule-based
ceiling (real neighbor distances/density a per-µm coefficient can't resolve) — which
is exactly what the field-solve work below closes.

**Field kernel (v1).** An `eps_r <value>` rule turns on a geometry-derived
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

**What the field-kernel study established (the honest ceiling).** Three independent levers —
fringe-corrected coupling, a global fringe cut, and conditional shielding — were each
measured against the OpenRCX golden, and **none tighten the ±40% per-net spread**. So
that spread is the **rule-based / analytic ceiling**: the calibrated model nails the
block total (~2 %) and is portable, but per-net accuracy below ±40 % depends on
detailed local geometry (exact multi-neighbour configurations, density) that no
global coefficient captures. Closing it requires **true per-net field solving**
against the actual layout — the research-grade endpoint commercial sign-off extractors occupy.

The road to sign-off grade is therefore a genuine **2.5-D field/pattern-matched
solver** per net (not more global coefficients). The distributed RC tree is now
built from the routing geometry; **moment-weighted** reduction and pin-access-accurate
attachment (reading pin locations from LEF/DEF `PINS` rather than binding pins to the
tree's leaf vertices) are the remaining tree refinements. Same file formats and CLI;
same `run` command, no license.

## For researchers — open problems

`vyges-extract` is a clean, std-only, fully open codebase with honest baselines and a
reproducible correlation harness — a good substrate for student research. Each item below
is a self-contained, publishable direction; the engine's file-in/file-out boundary means a
new method can be dropped in behind the same SPEF output and measured against the existing
baseline.

1. **2.5-D field / pattern-matched extraction.** Replace the analytic coupling kernel
   (`field.rs`) with a real field solve or a pattern-matched library per net. *Open
   question:* close the ±40 % per-net spread the analytic ceiling leaves, on open PDKs,
   without foundry data. Baseline + harness: [`correlation/openrcx-counter.md`].
2. **Calibration methodology to silicon.** A principled, documented procedure to correlate
   the rule/field coefficients to *measured silicon* (not just OpenRCX), with uncertainty
   bounds. *Publishable as:* an open calibration flow + dataset for sky130/gf180.
3. **Geometry-dependent resistance.** Width dependence ships (`R = rsheet × len /
   width`), with per-segment widths from non-default rules (`NONDEFAULTRULE` /
   `TAPERRULE`) now resolved by the DEF reader. Remaining: **via-array** (multi-cut)
   resistance, **per-layer / per-cut** via resistance (today one global `via` ohm), and
   **non-Manhattan** (corner / bend) wire resistance.
4. **Moment-weighted RC reduction.** Model-order reduction (AWE / PRIMA-style) of the
   distributed tree (`tree.rs`) to a compact, delay-accurate equivalent — and a study of
   accuracy vs. node count for STA.
5. **Pin-access-accurate attachment.** Bind parasitics to true pin-access locations from
   LEF/DEF `PINS` instead of the tree's leaf vertices, and quantify the delay impact.
6. **Spatial-scaling & parallelism.** The coupling grid (`coupling.rs`) scales with routed
   area; characterize it on full blocks and explore parallel / out-of-core extraction.

**Working on one of these — or want to?** We're actively pursuing several of these areas
ourselves, but the open frontier is bigger than any one team, and we'd rather build it in
the open than wait. If an item here fits your research, a student project, a thesis, or just
an itch, we'd genuinely like to hear from you — open an issue, send a PR, or reach us at
<https://vyges.com/contact>. Promising directions become collaborations; the correlation
harness makes before/after numbers easy to report in a paper.
