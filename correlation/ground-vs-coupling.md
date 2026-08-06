# Where the capacitance gap actually is — ground vs coupling, per net

- **Written:** 2026-08-01
- **Block:** `fft_ctrl_tlul`, 14 286 signal nets, sky130A, against the design's **own sign-off
  OpenRCX SPEF** (`fft_ctrl_tlul.nom.spef`).
- **Reproduce:** `python3 correlation/decompose.py --ref <sign-off>.spef --ours ours.json --def
  <routed>.def`
- **Prerequisite:** the RC topology had to be right first — see
  [`openrcx-diff-spef.md`](openrcx-diff-spef.md). Comparing magnitudes while 7 % of nets were
  emitted in disconnected pieces would have measured the wrong thing.

## The answer

Two separable errors, in opposite directions:

| | ours (fF) | sign-off (fF) | ratio |
| --- | ---: | ---: | ---: |
| ground | 83 071 | 60 653 | **1.37× over** |
| coupling | 29 728 | 90 085 | **0.33× — 3× under** |

Per net, both are systematic rather than noisy, and coupling is now an unusually clean single
scale — a p90/p10 span of 1.6, which is what a pure coefficient error looks like:

| | p10 | p25 | median | p75 | p90 |
| --- | ---: | ---: | ---: | ---: | ---: |
| ground | 1.09 | 1.25 | **1.47** | 1.74 | 2.08 |
| coupling | 0.26 | 0.30 | **0.33** | 0.37 | 0.41 |

### These are not the numbers this document first reported

The first run of this comparison found coupling **2.40× over**, with a wide per-net spread
(p10 1.65, p90 3.79). That was a **bug in the LEF reader**, not a model error, and chasing it is
what found it — see below. With it fixed, coupling inverts to 0.33× and the spread collapses.

The two errors had been **partially cancelling**: 1.37× on ground against a spurious 2.40× on
coupling made the total look like a plausible 1.81×. It is now 0.75×. A single total ratio would
have hidden this indefinitely, which is the argument for decomposing, made concrete.

## ⛔ Two earlier numbers were wrong, and here is why

**"2.83×" was an artefact of mismatched conventions.** OpenROAD writes `*D_NET <total>` as the sum
of the entries listed *under* that net, so each coupling cap lands in exactly one net's total —
their 60 653 ground + 45 043 coupling reproduces their 105 696 `*D_NET` sum exactly. Our writer
adds a net's coupling from **both** sides. Summing the two files' `*D_NET` columns therefore
compares theirs-counted-once against ours-counted-twice. On a like-for-like rule the total gap is
**1.81×**, not 2.83×. Do not quote 2.83 from anywhere.

**"74 % of the excess is coupling" went with it** — that excess was not real.

**"met4/met5 are the problem" is refuted.** Those layers' capacitance is an explicitly
uncalibrated placeholder, so they were the obvious suspect. Bucketing by how far up the stack each
net reaches says the opposite — the ground error *falls monotonically* as nets climb, and the nets
using the uncalibrated layers are the **closest** to the reference:

| stack reach | nets | ground median | coupling median |
| --- | ---: | ---: | ---: |
| up to met1 | 843 | **1.72** | 2.26 |
| up to met2 | 12 510 | 1.47 | 2.53 |
| up to met3 | 628 | 1.34 | 2.38 |
| up to met4 | 256 | **1.26** | 1.80 |

The placeholder is not driving this. The error lives in the **calibrated** layers.

## What the error actually is — shielding, not a length coefficient

The first reading of the bucket table was that ground carries an **additive per-node excess**,
since the ratio decays as nets lengthen. That is wrong, and the code says so: our grounded
capacitance is `Σ(segment length × layer fF/µm)` and nothing else — no per-node, per-via or
per-net term exists to be additive. The ratio moves with net size because the **layer mix** does.

Solving for the per-layer coefficients the reference implies (`fit_ground.py`, least squares over
14 238 nets with per-layer routed length as the design matrix) pointed at met2 — 48 % of the
routed length and apparently **1.58× too high**. Re-fitting it would have been the obvious move,
and would have been a mistake.

**The error is neighbour-dependent.** Field that terminates on a neighbour is field that did not
terminate on ground, and a fixed fF/µm cannot express that. Adding one shielding term
(`shield_test.py`, fitting `ref_ground ~ Σ len·c − k·ref_coupling`) does this:

| | per-layer only | **+ shielding** |
| --- | ---: | ---: |
| per-net ratio, median | 1.122 | **1.000** |
| spread, p90 − p10 | 0.747 | **0.398** |
| residual RMS | 2.06 fF | **1.40 fF** |
| implied met2 fF/µm | 0.0767 | **0.1009** (deck 0.1211) |

with **k = 0.387** — about 40 % of the coupled field coming at ground's expense, which is a
plausible charge-conservation figure rather than a number that only fits.

And the direct check: the correlation between a net's **coupling-to-ground ratio** and **how far
our deck over-states its ground** is **+0.940** across 13 990 nets. The nets we get wrong are
precisely the dense ones.

So met2's apparent 1.58× was largely shielding in disguise — met2 is the densest layer here, so
it is the most shielded. With shielding in the model its implied coefficient rises to within 20 %
of the deck, and met1's lands 4 % *below* the deck rather than above it.

**vyges-extract already implements this.** The deck's `shield_k` reduces grounded cap by
`shield_k · Cc_net` on exactly this reasoning — and `sky130A.vyges-extract.rules` does not set it,
so it is 0. The mechanism was built and never turned on.

### Which forces the order of the remaining work

The three terms are not independent, so fitting any one alone bakes in the others' errors:

1. **Coupling first.** `shield_k` multiplies *our* coupling, which is 2.40× too large — enabling
   shielding today would over-subtract by that factor. Fitting `k` against an inflated `Cc` would
   be fitting one error against another.
2. **Then shielding**, with `k` near the 0.387 the reference implies.
3. **Then the per-layer coefficients**, which only become meaningful once ground is
   density-aware. Re-fitting them now would produce a deck tuned to this block's density.

That is also the retrospective explanation for `openrcx-counter.md`: `counter` is sparse, so
almost nothing was shielded, so an unshielded deck fitted it to 0.997 — and had to over-predict
the moment it met a dense block.

## The coupling error was a three-micron wire

Coupling was chased the same way, and the trail is worth recording because every plausible
hypothesis was wrong before the real cause turned up:

1. **Neighbour search too generous?** No. Sweeping `couple_cutoff` from 2.0 um to 0.2 um moved
   the block total by 11 %. Nearly all coupling is nearest-neighbour, so an over-inclusive
   search was not it.
2. **Geometry double-counting?** No. `overlap_gap` already requires both segments parallel, so
   perpendicular crossings never enter the lateral term, and the grid pairing dedups candidates.
3. **A bare parallel-plate kernel missing the ground-competition fall-off?** `field.rs` has a
   fringe-corrected kernel the deck never enables, so this was the attractive answer. The
   arithmetic kills it: at met1's real height (~0.94 um) and a 0.20 um gap the Sakurai fall-off
   is `exp(-0.8/(0.2+7.53)) = 0.90`, giving 0.079 fF/um against the deck's effective 0.057 — the
   "correction" would have made coupling *worse*.
4. **The actual cause.** Re-implementing the identical formula in Python gave 16.7 fF for a net
   where the engine reported 144.7 and the reference 17.4. Same lengths (the two ground figures
   agree to 0.1 fF), same coefficients, same cutoff — so the divergence had to be an input.

**`Lef` reported met1's routing width as 3 um instead of 0.14 um.** A `LAYER` block's default
width is `WIDTH <n> ;`, but the `SPACINGTABLE` in the same block carries its own rows —
`WIDTH 0 0.14`, `WIDTH 3 0.28` — and the reader matched those too, last-write-wins.

It stayed invisible because **resistance only consults the width when the deck supplies a sheet
resistance**, and the sky130A deck does not. Coupling always consults it, and `gap = centre -
width` with a 3 um width makes every edge-to-edge gap negative — so every parallel neighbour
clamps to the full coefficient and the distance cutoff never fires. That is also why the sweep in
(1) looked so flat: the cutoff was dead code on this input.

Fixed in `vyges-tools/loom 9f75a2c`, with the arity made exact — a row carrying more than one
value is a table entry, not a declaration.

## Why the deck did not catch this

## Also found: our net names do not round-trip

The join initially matched only 13 471 of 14 238 nets. Not a modelling difference — **escaping**.
The reference writes `u_adapter\.req_addr_q\[0\]`; we write it raw. Same net.

OpenRCX reads our file without complaint, so this is not a parse failure, but the names do not
round-trip against the incumbent's own output, and the 767 nets it silently dropped were exactly
the hierarchical ones — not a random sample. `decompose.py` normalises both sides; the writer
itself is unchanged so far. Worth fixing, and worth noting that a name-keyed comparison is one of
the ways this would have gone quietly wrong.

## ✅ Coupling is now fitted — on a set, with a held-out block

`correlation/calibrate_coupling.py` fits the per-layer coupling coefficients the same way
`calibrate.py` fits ground: coupling is linear in a layer's coefficient, so run the **real
extractor** once per layer with that layer at 1.0 and the rest at 0, and the per-net result is
that layer's design-matrix column. No re-implementation to drift.

The calibration set is 10 sky130 blocks, each with the OpenRCX SPEF its own OpenLane run
produced — 35 682 nets, from `counter` (50) to `mag_phase_apb` (9 917). **`fft_ctrl_tlul`
(14 238 nets) was held out entirely** and only scored afterwards:

| | before | after |
| --- | ---: | ---: |
| held-out block, coupling total | 0.33× | **1.02×** |
| held-out block, per-net median | 0.33 | **1.04** (p10 0.83, p90 1.24) |
| every fitted block, per-net median | 0.29 – 0.33 | **0.93 – 1.03** |
| held-out block, **total** capacitance | 0.75× | **1.16×** |

| layer | deck | fitted | constraining nets |
| --- | ---: | ---: | ---: |
| met1 | 0.0807 | **0.21941** | 33 035 |
| met2 | 0.0740 | **0.26618** | 31 282 |
| met3 | 0.0806 | **0.26458** | 711 |
| met4 / met5 | 0.0806 | met3's value | 92 / 0 — **not fitted** |
| li1 | 0.030 | unchanged | 0 — **not fitted** |

A least-squares fit hands back **0.0** for a layer nothing constrains, and writing that would
silently switch its coupling off. The script refuses coefficients below 200 constraining nets and
carries a stated placeholder instead.

### ⚠️ The fitted number is not physical, and that is informative

At minimum spacing the fitted met1 coefficient works out to ~0.153 fF/µm against an ideal
sidewall parallel-plate value of ~0.062 — **2.5× a bound it cannot exceed on physics**. The
explanation is that this deck defines **no `interlayer` coefficients**, so the model computes
*zero* crossover coupling and the lateral term is absorbing it.

So the fit reproduces OpenRCX's per-net coupling across eleven blocks while being wrong
per-mechanism. It will hold while the crossover-to-lateral ratio resembles these blocks — all
sky130 std-cell digital — and should not be trusted on a design with markedly different layer
usage. That bound is now written into the deck header rather than left to be rediscovered.

## ✅ And ground, with shielding turned on

`correlation/calibrate_ground.py` fits the model the deck can actually express, both terms at
once and on the same set:

    ground(net) = Σ_L( length_L × c_L )  −  k × Cc(net)

Run **after** the coupling fit, not before: `k` multiplies *our* coupling, so fitting it against
a column that is itself 3× wrong is fitting one error against another.

| | before | after |
| --- | ---: | ---: |
| held-out block, ground total | 1.37× | **0.98×** |
| held-out block, ground per-net median | 1.47 | **0.95** (p10 0.73, p90 1.20) |
| every fitted block, per-net median | 1.25 – 1.47 | **0.96 – 1.01** |

**`shield_k` = 0.181.** That reconciles with the 0.387 the reference's own numbers implied: that
estimate used a net's coupling counted **once**, this fit uses the both-sides sum `engine.rs`
actually subtracts, and 0.387/2 ≈ 0.19. Same physical fraction, columns differing by two — worth
stating, because an unexplained 2× between two estimates of the same thing is exactly the kind of
discrepancy that should not be left sitting in a doc.

The per-layer coefficients barely moved (met1 ×0.99, met3 ×0.97); **met2 came down 20 %**, which
is the same met2 the no-shielding fit wanted to cut by 1.58×. The shielding term did the work,
and it confirms the diagnosis: met2 is the densest layer here, so it was the most mis-attributed.

## Where the deck now stands

On `fft_ctrl_tlul`, **held out of every fit**:

| | ratio |
| --- | ---: |
| ground | **0.98×** |
| coupling | **1.02×** |
| **total** | **1.00×** |

Started at ground 1.37× / coupling 0.33× / total 0.75×, with a topology that had 7 % of nets in
disconnected pieces.

## ⚠️ Crossover: identifiable after all, and it still does not settle the mechanism

The stated reason for the coupling coefficient being ~2.5× a sidewall parallel-plate bound was
that the deck has no `interlayer` term, so crossover coupling is computed as zero and the lateral
coefficient absorbs it. That is a hypothesis, and it has now been tested twice.

**First attempt — the fit refused the term.** `calibrate_coupling.py` fits lateral **and**
crossover jointly (one coefficient per layer, one per adjacent layer pair, all competing for the
same reference coupling). On the original 11-block set it drove every significant crossover pair
to **0.0** and made the held-out block marginally worse. The reason was not physics: the two
columns are **0.93–0.96 correlated** there (`collinearity.py`), so least squares cannot separate
them and zeroing one is what it does.

**That was blamed on the wrong thing.** The conclusion drawn was "we need a block that is not
sky130 std-cell digital". Wrong twice over:

- **Layer mix is nearly invariant** across these designs — `openframe_project_wrapper` has 6× the
  routing of `fft_ctrl_tlul` (4.19 M µm) and almost the same profile (met1 39 % / met2 41 % /
  met3 12 % vs 44 / 41 / 9). Same PDK, same router, same preferred directions.
- **But collinearity is not.** On the wrapper and `xbar_mem` it is **+0.62**, not +0.93. The
  blocks that break the degeneracy were already in hand; the layer mix simply does not predict
  which those are, and inferring it from the mix was a mistake.

**Second attempt — with those blocks in, crossover is identified.** Adding
`openframe_project_wrapper`, `picorv32` and `xbar_mem` (14 blocks, 53 522 nets):

| | lateral-only | joint |
| --- | ---: | ---: |
| met1 lateral | 0.2219 | **0.1516** |
| met2 lateral | 0.2706 | **0.2260** |
| met1/met2 crossover | — | **1.719** (54 015 nets) |
| met2/met3 crossover | — | **2.397** (22 012 nets) |

The lateral coefficient drops ~30 % once crossover can carry some of the load, so the "lateral is
absorbing crossover" hypothesis is **partly supported**. But:

1. **The crossover coefficient is ~25× a parallel-plate estimate.** At met1/met2 spacing
   (d ≈ 0.45–0.65 µm, εr 3.9–4.2) the plate value is 0.053–0.083 fF/µm²; the fit wants 1.72.
2. **Prediction does not improve.** Held-out median 1.04 → 1.01, but p10 0.83 → 0.76 and p90
   1.24 → 1.33. `xbar_mem` degrades badly: p90 1.22 → 2.06. Centres shift slightly, spreads widen.

So the joint model is **curve-fitting, not mechanism recovery** — it buys a more physical lateral
coefficient with a wildly unphysical crossover one and a worse spread. **It was not shipped.**

The honest state: the totals are calibrated and validated; the mechanism is not established; and
it is now known that "crossover is simply missing" does not explain it either. Something else is
inflating the coupling a real extractor reports relative to what a sidewall model predicts, and
neither of the two obvious candidates — an over-inclusive neighbour search, a missing crossover
term — survives contact with the data.

## Coupling to power: ruled out in one command

The next candidate was that the reference books signal-to-power-rail adjacency as net-to-net
coupling. This engine reads `d.nets` (DEF signal nets) and never sees `SPECIALNETS`, so a whole
class of neighbour would be invisible and the lateral coefficient would have to compensate.

**The reference SPEF contains no power nets at all** — zero matches for VPWR/VGND/VDD/VSS, and
14 238 `*D_NET` records against the DEF's 14 286 signal nets and 2 special nets. OpenRCX put that
field into grounded capacitance, which is what a rail that is an AC ground deserves. So there is
no power coupling in the reference for our lateral term to be matching. Dead, at the cost of one
`grep`.

## What the pair-level comparison actually shows

`pair_compare.py` compares the two files **net pair by net pair**, which is the only way to tell
a model that is right from two errors that cancel. On the held-out block with the shipped deck:

| | pairs | ours (fF) | reference (fF) |
| --- | ---: | ---: | ---: |
| found by both | 78 811 | 40 447 | 43 932 |
| only ours | 52 364 | 5 707 | — |
| only the reference | 10 029 | — | 1 110 |

and on the shared pairs, per-pair ratio p10 0.59, **median 0.82**, p90 2.18.

So the 1.02× total is **not** a clean match. It is:

- **we are ~8 % low on the pairs we both find**, and 18 % low at the median;
- **long on pairs** — 52 364 the reference does not report. Two thirds of them (35 018, 1 554 fF)
  sit below OpenRCX's 0.1 fF coupling threshold and are *expected* to be absent, since it lumps
  those into ground. The other **17 346 pairs, 4 153 fF (9 % of our coupling)** are adjacency we
  claim and it does not;
- **short on 10 029 pairs** worth 1 110 fF that it finds and we do not.

Those roughly cancel. That is a materially better statement of the open problem than "the
coefficient is 2.5× a parallel-plate bound": the coefficient is inflated **because it is fitted to
recover a total from a pair set that is both too large and individually too small**.

It also gives the next question a shape: the largest pairs we claim and the reference does not are
tens of fF between long parallel nets (`_00990_`/`_01006_` at 31 fF). Either those wires are not
adjacent the way our geometry thinks, or the reference is attributing that field somewhere else.
One net, two files, decidable.

## The cutoff sweep, re-run — and the yardstick was the problem

The largest disputed pair (`_00990_`/`_01006_`, 31 fF we claim and the reference does not) turned
out to sit at a **0.88 µm gap**, not minimum spacing — two or three tracks away, with wires in
between. The obvious reading was that we lack screening: a real extractor knows the intervening
metal shields that pair, and we keep counting it through a bare `1/gap` fall-off.

Sweeping `couple_cutoff` says otherwise. (The first sweep of this, which showed the cutoff barely
mattering, ran while the LEF reader reported met1 as 3 µm wide — every gap was negative, so the
cutoff could never trip. It was re-run against the fixed reader.)

| cutoff | shared pairs | only ours (≥0.1 fF) | only the reference | per-pair median |
| ---: | ---: | ---: | ---: | ---: |
| 2.0 µm | 78 811 | 17 346 / 4 153 fF | 10 029 / 1 110 fF | 0.82 |
| 1.0 µm | 66 623 | 5 744 / 1 398 fF | 22 217 / 3 302 fF | 0.79 |
| 0.5 µm | 42 495 | **3 / 1 fF** | **46 345 / 10 557 fF** | 0.77 |
| 0.25 µm | 23 871 | 0 | 64 969 / 26 002 fF | 0.73 |

Tightening the cutoff does remove our extra pairs — and immediately loses far more of the
reference's. **The reference reports coupling well past 1 µm of gap**; it is not screening those
neighbours out, so our 2.0 µm reach is right and the screening story is wrong.

What survives every cutoff is the interesting part: **the per-pair median sits at 0.77–0.82
throughout**. On the pairs we share we are uniformly ~20 % low, and no pairing change touches it.

### Which means the "2.5× a parallel plate" anomaly may not be one

That figure compared our *fitted lump* against a bare sidewall parallel-plate bound. But the
fitted coefficient is, by construction, **the coefficient the reference behaves as if it used** —
`calibrate_coupling.py` regresses the reference's own per-net coupling onto our geometry. If
0.219 fF/µm is unphysical, then so is the foundry-reference extractor, on its own numbers.

The likelier reading is that the bound was the wrong yardstick: a bare plate ignores fringe from
the top and bottom of the sidewall, and our `s_ref/max(gap, s_ref)` shape clamps at 0.14 µm while
the common routed gap is 0.20 µm, so the coefficient must run ~1.4× high just to reproduce the
same effective value. Those two together account for most of the gap; they do not obviously
account for all of it.

So the honest state is weaker than "the mechanism is wrong" and weaker than "everything is fine":
**we are uniformly ~20 % low per pair, the pair set is close but not identical (89 % of the
reference's, missing 2.5 % of its coupling — plausibly the crossover we compute as zero), and the
physical objection to the coefficient rests on a bound that does not clearly apply.**

## Resolved, 2026-08-06 — and the largest single error was in this harness

Everything above about "the coefficient is ~2.5× a parallel-plate bound" was chasing an artefact.
**This harness was double-counting the reference.**

OpenRCX writes each coupling cap into **both** nets' SPEF blocks, at full value, so that a net's
block is self-contained — `extSpef::writeSrcCouplingCaps` and `writeTgtCouplingCaps`. Measured on
`counter`: 350 coupling lines, 175 distinct caps, exactly 2.00 listings each; on `fft_ctrl_tlul`:
268 936 lines, 134 468 caps. `decompose.parse_ref` credited both ends of **every listing**, so
every reference coupling value in this document — and every coefficient ever fitted against one —
was 2× the real value. Every published *ratio* still looked right, because the deck was fitted to
the doubled reference and doubled with it. Two errors cancelling.

Both readers now count each node pair once.

Two model errors were corrected at the same time, both taken from the reference's source rather
than inferred from measurements:

1. **The fall-off is characterised, not `1/s`.** Each layer's `couple_shape` curve is that layer's
   own DIST table, normalised to its first sample, interpolated as `extDistRCTable::getComputeRC`
   does. met1 keeps 0.773 of its minimum-spacing coupling at twice the spacing; `1/s` says 0.500.
   This was the uniform ~20 % per-pair deficit, in full.
2. **A wire couples only to what it can see, shadowed by coverage.** The reference carries each
   wire as a set of uncovered pieces and walks outward one track at a time; a neighbour consumes
   exactly the run length it spans, and the remainder stays visible to wires further out
   (`Track::findOverlap`). Power wires take part as metal that blocks and never couples — their
   field is booked to ground, which is why the reference SPEF has no power nets. Without this we
   reported 52 364 pairs the reference does not; with it, 387.

### The check that needed no fitting

Load OpenRCX's own characterised per-layer coefficients and curves into the deck, run our
geometry, count the reference correctly:

| block | shared pairs | ours fF | reference fF | p10 | median | p90 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `counter` | 94 | 13.8 | 13.5 | 0.87 | **0.98** | 1.33 |
| `fft_ctrl_tlul` | 76 903 | 23 009 | 21 884 | 0.90 | **1.03** | 1.27 |

Zero free parameters.

### What the deck ships now

Re-fitted on 14 blocks with `fft_ctrl_tlul` held out, the coupling coefficients land on the
reference's own characterised values — met1 **0.0932** against **0.1055** (0.88×), met2 1.04×,
met3 1.02× — and the held-out spread is the tightest of any configuration tried:

| held-out `fft_ctrl_tlul`, coupling | p10 | median | p90 | met1 ÷ characterised |
| --- | ---: | ---: | ---: | ---: |
| previous deck (1/s, no visibility rule) | 0.83 | 1.04 | 1.24 | 2.08× |
| **this deck** | **0.89** | **1.00** | **1.13** | **0.88×** |

Ground is unchanged: the ground columns are carried over and `shield_k` is doubled, so the amount
subtracted is the same now that the coupling column has halved (ground 0.98× total, median 0.96).
Re-fitting ground from scratch was tried and scored worse on the held-out block (0.89× total), so
the algebraic carry is what ships.

**The lesson:** the discrepancy was a clean, gap-independent factor of ~2 with a tight spread, and
it was read as physics for a month. When a gap is a constant factor, audit the comparison before
the model.

## Next

1. **Cross-layer (diagonal) coupling.** The reference reports 11 937 pairs we do not, worth 2.9 %
   of its coupling. Its mechanism is the `DIAGUNDER` tables under `DIAGMODEL ON`, which book field
   between a wire and same-direction wires on other layers as real coupling, by diagonal distance.
   Our `interlayer` term is an areal-overlap model and cannot represent it — which is why fitting
   it jointly always produced a ~25× parallel-plate coefficient.
2. **Ground has become the weaker axis** (p10 0.77 / p90 1.20 against coupling's 0.89 / 1.13).
   One identified mechanism: the reference books rail-adjacent and sub-threshold field *to ground*,
   and we now correctly exclude both from coupling without adding them anywhere.
3. **Escape hierarchical net names in the SPEF writer** (above) — small, and it is the difference
   between a name-keyed comparison working and silently dropping 5 % of nets.
4. Resistance has never been correlated at all. Capacitance is now two terms deep; R is still the
   tech-LEF value taken on faith.
