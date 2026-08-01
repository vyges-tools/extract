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

| | ours (fF) | sign-off (fF) | ratio |
| --- | ---: | ---: | ---: |
| ground | 83 071 | 60 653 | **1.37×** |
| coupling | 215 839 | 90 085 | **2.40×** |

**74 % of the excess capacitance is coupling** (62 877 fF of 85 295 fF, counting each physical
capacitor once). Coupling is also the larger half of our total in absolute terms. If only one
thing gets fixed, it is the lateral kernel — not the per-layer ground coefficients.

Per net, both errors are systematic rather than noisy — we are over on nearly every net, not
wrong on a few:

| | p10 | p25 | median | p75 | p90 |
| --- | ---: | ---: | ---: | ---: | ---: |
| ground | 1.09 | 1.25 | **1.47** | 1.74 | 2.08 |
| coupling | 1.65 | 2.05 | **2.50** | 3.03 | 3.79 |

## ⛔ Two earlier numbers were wrong, and here is why

**"2.83×" was an artefact of mismatched conventions.** OpenROAD writes `*D_NET <total>` as the sum
of the entries listed *under* that net, so each coupling cap lands in exactly one net's total —
their 60 653 ground + 45 043 coupling reproduces their 105 696 `*D_NET` sum exactly. Our writer
adds a net's coupling from **both** sides. Summing the two files' `*D_NET` columns therefore
compares theirs-counted-once against ours-counted-twice. On a like-for-like rule the total gap is
**1.81×**, not 2.83×. Do not quote 2.83 from anywhere.

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

## What the shape of the error suggests

Two different signatures, so probably two different causes:

- **Ground gets worse as nets get shorter** (met1-only local nets are 1.72×, tall nets 1.26×). A
  pure per-µm coefficient error would be flat with length; a ratio that decays as nets lengthen is
  the signature of an **additive, per-net or per-node excess** — something charged once per node
  or per via landing rather than per micron. Worth looking at what the li1/met1 stubs and via
  landings contribute before touching any coefficient.
- **Coupling is roughly flat at ~2.4–2.5× across every bucket.** That is the signature of a
  **scale error in the lateral kernel**, not a structural or neighbour-search one — a wrong
  neighbour set would vary wildly with local density, and this does not.

## Why the deck did not catch this

[`openrcx-counter.md`](openrcx-counter.md) reports the calibrated deck tracking OpenRCX to
**0.997 on total capacitance**. That is not contradicted here, and it is also not reassuring:
`calibrate.py` fits **per-layer ground-cap scales** against OpenRCX's per-net totals on `counter`
— 50 nets, sparse, where coupling is a rounding error. **The coupling model was never fitted to
anything.** On a dense block coupling is 72 % of our total capacitance, so the one term the
calibration never touched is now the dominant one.

That is the concrete gap behind the "fit on **one** representative block" caveat that doc has
carried from the start, and it is sharper than "accuracy degrades off the calibration set": a
whole term of the model is unconstrained.

## Also found: our net names do not round-trip

The join initially matched only 13 471 of 14 238 nets. Not a modelling difference — **escaping**.
The reference writes `u_adapter\.req_addr_q\[0\]`; we write it raw. Same net.

OpenRCX reads our file without complaint, so this is not a parse failure, but the names do not
round-trip against the incumbent's own output, and the 767 nets it silently dropped were exactly
the hierarchical ones — not a random sample. `decompose.py` normalises both sides; the writer
itself is unchanged so far. Worth fixing, and worth noting that a name-keyed comparison is one of
the ways this would have gone quietly wrong.

## Next

1. **Fit the coupling coefficient.** It is 74 % of the error and its flat ratio says a single
   scale may carry most of it. `calibrate.py` needs to solve for coupling as well as ground,
   which means fitting against a block where coupling is not negligible — i.e. not `counter`.
2. **Find the additive term in ground** before re-fitting per-layer scales, or the fit will
   absorb a per-node error into a per-µm coefficient and look right on one block again.
3. Only then re-fit the deck across a **set** of blocks.
