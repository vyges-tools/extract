# Calibrated parasitics — vyges-extract vs OpenRCX on the routed counter

Calibrating vyges-extract's open **sky130A** deck against the foundry-reference
extractor (**OpenROAD OpenRCX**) on a real placed-and-routed block: the 8-bit counter
through OpenLane. The same routed DEF feeds both; we compare per-net capacitance.

## The block

`counter` routed through OpenLane, 50 signal nets on
li1 / met1 / met2 / met3. OpenRCX golden = the run's own
`final/spef/nom/counter.nom.spef` (model `rules.openrcx.sky130A.nom`).

## Calibration method (`calibrate.py`)

A deck calibration is just fitting the per-layer coefficients so the extractor's
output matches the reference — exactly what a per-fab plugin does, here against
OpenRCX instead of silicon:

1. **Isolate** each layer's contribution — run vyges-extract once per layer with only
   that layer's cap active → a per-net design matrix `A` (net × layer).
2. **Regress** the OpenRCX per-net total `b` against `A` (non-negative least squares)
   → a per-layer cap scale `α_L`.
3. **Emit** the calibrated deck = illustrative caps × `α_L`.
4. **Validate** — re-extract with the calibrated deck and correlate.

li1 is **pinned** to its physical value before the fit: every li1 net also runs on
met1, so an unconstrained fit folds li1's (tiny) cap into met1 and zeroes it — a
single-block collinearity a real multi-block calibration set breaks. Pinning li1
changes the result by nothing (its cap is genuinely small), and keeps the deck
physical.

## Result — total within 0.3%, tight per-net spread

Fitted scales: met1 ×1.61, met2 ×1.68, met3 ×2.02 (the illustrative caps undercounted
fringe + vertical coupling); li1 pinned ×1.0.

| deck | total cap ratio | per-net mean | median | spread (min–max) | σ |
| --- | --- | --- | --- | --- | --- |
| illustrative (uncalibrated) | 0.60 | — | 0.59 | 0.51 – 0.79 | — |
| **calibrated (fit to OpenRCX nom)** | **0.997** | **1.017** | **0.975** | 0.823 – 1.328 | 0.127 |

Calibrated total tracks OpenRCX to **0.997**; per-net mean ~1.0 with a tight ±13%
spread (was a systematic ~0.6× undercount). The calibrated met caps land at
~0.12–0.14 fF/µm — physical sky130 total-wire values vs the illustrative 0.078.

## Honest bounds

- Calibrated to **OpenRCX** (a reference extractor), not silicon. A sign-off /
  certified per-fab deck is **silicon-correlated** and **NDA** — never in-repo.
- Fit on **one** representative block; a production deck calibrates on a SET (breaks
  per-layer collinearity, covers more topologies).
- The residual ±13% per-net spread is the **per-µm model gap** — vyges-extract lumps
  the width/spacing dependence that OpenRCX's tables resolve. A width/spacing-aware
  deck is the next depth step.
- **Capacitance** is calibrated here; **resistance** keeps the illustrative per-µm
  value (an R-correlation pass is separate).

**Follow-up (2026-08-01):** taking this deck to a 285x larger block found that it cannot extract
it at all — `counter` never routes above met3, so the fit produced no met4/met5 entries. See
[`openrcx-diff-spef.md`](openrcx-diff-spef.md), which also records three interop defects that
made every SPEF this engine wrote unreadable by OpenSTA. The "fit on one block" caveat below was
right and understated.

Reproduce: `python3 correlation/calibrate.py` (needs the routed DEF/LEF + the OpenRCX
nom SPEF; paths at the top of the script). The emitted deck is
`pdk/sky130A/sky130A.vyges-extract.rules`.
