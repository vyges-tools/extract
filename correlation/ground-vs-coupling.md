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

## Next

1. **Fit the coupling coefficient.** Now a well-conditioned target: a near-uniform 0.33x with a
   p90/p10 span of 1.6, so a single scale should carry most of it. `calibrate.py` must solve for
   coupling, against a block where coupling is not negligible — i.e. not `counter`.
2. **Turn on `shield_k`** and fit it, once `Cc` is trustworthy. Expect ~0.39.
3. **Re-fit the per-layer ground coefficients** last, with shielding active, across a **set** of
   blocks. Doing this first would tune the deck to one block's density.
