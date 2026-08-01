# Correlating against OpenRCX with `diff_spef` — the harness, and what it found first

- **Written:** 2026-08-01
- **Status:** harness built; OpenRCX now **reads our SPEF completely**, which it could not do at
  all before. **No numeric correlation is obtainable this way** — `diff_spef.out` is unreachable
  in OpenROAD as pinned, verified in source. What the exercise *did* produce: **eight** writer
  defects, all fixed, and a real extraction bug — **7 % of nets emit an RC network in more than
  one piece**.
- **Companion:** [`openrcx-counter.md`](openrcx-counter.md) — the per-layer cap calibration this
  was meant to generalise.

## Why `diff_spef` rather than our own comparator

`openrcx-counter.md` correlated against OpenRCX using `calibrate.py`, our own script. That
leaves "the comparison could be wrong" as a live variable. OpenROAD ships `diff_spef`, so the
incumbent supplies both the reference *and* the definition of a difference, and we supply only
the file being judged. Same reasoning as using a sign-off SDF for the timer rather than a
comparison of our own devising.

One thing to know before copying this: **`diff_spef` compares a file against the in-memory
extraction database, not against another file.** OpenRCX has to actually run in the same process.
That is a feature — the reference is produced here, from the same DEF, with the same PDK model
the design's own sign-off used (`rules.openrcx.sky130A.nom.spef_extractor`).

```tcl
read_liberty …; read_lef …; read_def fft_ctrl_tlul.def
define_process_corner -ext_model_index 0 X
extract_parasitics -ext_model_file rules.openrcx.sky130A.nom.spef_extractor -corner_cnt 1
diff_spef -file ours.spef -r_res -r_cap -r_cc_cap
```

## What it found before producing a single number

The harness was pointed at `fft_ctrl_tlul` — 14 286 nets against the calibrated block's 50, so
**285× the size of anything this extractor had been checked on**. Three defects surfaced in the
first ten minutes, all of them invisible to our own test suite because our own reader accepts
our own output.

### 1. Our SPEF was unreadable by OpenSTA — ✅ fixed

`[ERROR STA-1670] ours.spef line 3, syntax error`. **`*DATE` and `*DESIGN_FLOW` are required**
by the SPEF grammar OpenSTA implements, and we emitted neither. Every SPEF this engine has ever
written was rejected outright by OpenROAD, LibreLane, and anything built on them.

`*DATE` was omitted *deliberately* — a writer test asserted its absence, to keep output
byte-reproducible. That requirement is real; dropping a mandatory field was the wrong way to
meet it. A **fixed** stamp satisfies both, and the test now asserts the field is present *and*
that two runs are byte-identical.

The same hole existed in loom's writer (`vyges-tools/loom 1ffc3a1`) — two SPEF writers, one
defect, which is its own finding.

### 2. Every port lost its parasitic connection — ✅ fixed

`[WARNING STA-1648] instance PIN:tl_o[40] not found`, once per port. DEF spells a top-level port
connection with the pseudo-instance `PIN`; SPEF spells one **`*P <name> <dir>`**, not
`*I <inst>:<pin>`. We emitted everything as `*I`, so a reader hunted for an instance literally
named `PIN` and dropped the connection. **322 ports** on this block. Now emitted as `*P`.

### 3. The calibrated deck could not extract the block at all — ⚠️ partly addressed

`error: no rule for layer "met4"`. The deck was fit on `counter`, which never routes above met3,
so it has **no met4/met5 entries** — and `fft_ctrl_tlul` uses both. This is the generalisation
limit `openrcx-counter.md` predicted ("fit on **one** representative block"), and it is sharper
than that doc implies: the deck does not merely lose accuracy off its calibration set, it
**refuses to run**.

Extended with met4/met5: **resistance is physical** (from the same tech LEF the calibrated layers
took theirs from), **capacitance is met3's value standing in and is NOT calibrated**. On this
block met4+met5 are 565 of ~230 000 routed segments (0.24 %), so the effect on a total is small
— but it is a placeholder and any number quoted from a design that leans on those layers has to
say so.

**Worth noting:** the `EXTRACT-LAYERS` coverage event added earlier the same day printed
`5 used by the routing, 3 with RC rules` *one line before* the failure. Its first outing on a
real design predicted the failure it was written to make visible.

## ✅ The segfault is gone — and it was ours (2026-08-01, later)

Isolated by bisecting **our own SPEF** rather than finding a smaller design. `diff_spef` does not
require the file to cover every net, so a prefix is a valid SPEF; and OpenRCX's extraction can be
run **once** and frozen with `write_db`, after which each bisect step is a `read_db` instead of a
4-second re-extraction of a 240 000-instance design. Binary search over 14 286 records, ~14 runs.

That named the trigger in one pass, and then again after each fix. Four more defects, all ours:

| # | Defect | Why it was invisible to us |
| --- | --- | --- |
| 4 | The DEF `PIN` placeholder was still **interned into `*NAME_MAP`** after ports moved to `*P` | our reader never resolves name-map entries as instances; OpenRCX does |
| 5 | **Coupling entries named a port as an instance pin** (`*<id>:tl_i[74]`) | a dangling reference our reader tolerates |
| 6 | Node labels **prefixed `*` on a port** — which marks a *name-map reference*, so the reader looked up an id called `tl_i[74]` | ditto |
| 7 | **No `*PORTS` section**, and it must come **after** `*NAME_MAP` — a reader meeting it first concludes there is no name map at all | we never read our own ports back |

**Defect 4 was masking the crash.** It produced a clean `Unmatched spef and db` error that stopped
OpenRCX before it reached the null dereference; fixing our bug is what exposed the segfault. So
the honest reading is: the crash is an upstream robustness bug (`getDbInst` → `NameTable::getDataId`
on an unresolved name, no null check), *and* every input that triggered it was malformed by us.
Worth reporting upstream with the reproducer; not worth blaming for the delay.

**Result: OpenRCX now reads our SPEF completely** —
`Have read 14286 D_NET nets, 137520 resistors, 157182 gnd caps, 243668 coupling caps`.

### Defect 8, and the last one blocking the read: a port node written `*0:<port>`

`RCX-0044 1 spef insts not found in db` → `RCX-0052 Unmatched spef and db`, which aborts before
any comparison. The count says *one*, so it reads like a corner case; it was 548 references.

`emit_tree` labelled a node's instance side with `nm.id(inst).unwrap_or(0)`. A port has no
instance side — DEF spells it with the `PIN` placeholder, which we deliberately never intern —
so the fallback minted `*0:clk_i`, a name-map reference to an id the map has no entry for. The
star path had the same hole from the other end of the same sentinel (`*<usize::MAX>:…`).

Both are now one `node_label` helper, and **labels carry their own `*` rather than having one
concatenated on at the emission site** — that split is what let the two disagree. The test
asserts the *rule* (every `*<id>` in the body resolves) rather than any of the four symptoms,
since asserting any one of them would have caught none of the others.

With that fixed, `diff_spef` runs to completion: `RCX-0044` and `RCX-0052` are gone, exit 0.

## ⛔ The numeric report cannot be produced — `diff_spef.out` is unreachable upstream

This is the finding that matters for the plan, and it is not about our file.

`diff_spef` completes, and writes nothing. The report is written through `_diffOutFP`, opened in
exactly one place:

```cpp
// extSpef.cpp:228
void extSpef::setUseIdsFlag(const bool diff, const bool calib) {
  _diff = diff;
  if (diff && !calib) { _diffLogFP = fopen("diff_spef.log", "w");
                        _diffOutFP = fopen("diff_spef.out", "w"); }
}
```

Across **all of OpenROAD** at the pinned SHA (`b5624809`) there are exactly two calls:
`netRC.cpp:2116` passes `false`, and `extSpefIn.cpp:2478` passes `(true /*diff*/, true /*calib*/)`
— which fails `diff && !calib`. `_diffOutFP` is `nullptr` at declaration and assigned nowhere
else. **No input can make OpenROAD write `diff_spef.out`.**

And the one call site that could reach it is itself unreachable from `diff_spef`: it is guarded by
`_db_calibbase_corner >= 0`, which is set from `readSPEF`'s `calibrateBaseCorner` parameter.
`Ext::read_spef` forwards `opt.calibrate_base_corner` (`ext.cpp:337`). `Ext::diff_spef` passes
**`nullptr`** in that position (`ext.cpp:388`) — the option is defined in `DiffOptions`, plumbed
through, and dropped at the call site.

Nor is any of this exercised: the only reference in OpenROAD's test tree is a helper in
`rcx/test/rcx_aux.py` that reads `opts.file = file` — a free variable, not the `filename`
parameter — and no test calls it. **`diff_spef` has no regression coverage at all**, which is the
same fact that explains the segfault: an unexercised path had no reason to be robust.

So the harness premise — *let the incumbent define what a difference is* — **does not hold for
`diff_spef`**. What the command can still do, and did, is act as a **conformance reader**: eight
defects in our writer, found only by feeding our output back. That is worth keeping. It is not a
correlation.

**Getting an actual number needs the other route**: have OpenRCX `write_spef` its own extraction
and compare per-net values, which is what `openrcx-counter.md` already does with `calibrate.py`.
The comparator is then ours again — but both sides' *values* come from named tools and the
comparison is a ratio anyone can re-derive, which is a weaker claim than `diff_spef` promised and
a defensible one.

## ⚠️ What the reader does say about our RC — 7 % of nets are not one network

`diff_spef` cannot give values, but its topology checks run, and they found something real:

| | count | of 14 286 nets |
| --- | ---: | --- |
| `RCX-0272` RC **disconnected** | **1 001** | 7.0 % |
| `RCX-0374` RC **inconsistency** | 630 | 4.4 % |
| `RCX-0292` **looped** spef RC | 23 | 0.2 % |

All 1 001 are internal `_NNNNN_` nets, not ports, and they are multi-pin: the first three carry
4, 7 and 7 pins. Taking `_00768_` (4 pins, 19 grounded-cap nodes, 16 resistors) and running
union-find over its own `*RES` edges:

```text
3 connected components:
   15 nodes   *15828:Y is NOT among them          <- driver + 2 sinks
    3 nodes   *15827:A, *15828:Y                  <- the driver, stranded with one sink
    4 nodes   *769:15 *769:16 *769:17 *769:18     <- internal nodes reachable from nothing
```

The network we emit is **three graphs, not one**. Two sinks have no resistive path to the driver
at all, and four internal nodes float free. A timer reading this sees either zero interconnect
delay to those pins or an unsolvable network — so this is not a reporting nicety, it is wrong
parasitics on one net in fourteen.

This is a `tree::build_network` defect (routing segments not being stitched through the vias or
touch-points that join them), it is squarely extraction work rather than harness work, and it is
the next thing to fix. It is also the one finding here that a value comparison would have *hidden*
— per-net totals can be perfectly correlated while the network they describe is in pieces.

## (Historical) The blocker, when it was still unexplained: `diff_spef` segfaults reading our file

With the header and port defects fixed, OpenSTA parses our 17 MB SPEF. **OpenRCX's own SPEF
reader — a different reader — crashes:**

```text
rcx::NameTable::getDataId → rcx::extSpef::getDbInst → rcx::extSpef::getCapNodeId
  → rcx::extSpef::readDNet → rcx::Ext::diff_spef        SIGSEGV
```

Unresolved. Two things are true and neither is yet distinguished: our file may still contain
something OpenRCX's reader does not expect, **and** a parser that segfaults rather than erroring
on unexpected input is an upstream robustness bug. Note this is the **third** upstream segfault
this thread has hit (`writeDbv` in `lefout::writeLib` was the other), which is its own data point
about the maturity of the paths off the main flow.

Isolating it on a small design is not currently possible: `examples/counter/` is a geometry-only
fixture whose LEF has no macros, so OpenROAD cannot load the DEF at all. **A real small routed
sky130 block is the missing piece** — for this and for any future OpenROAD-side comparison.

## Where this leaves the correlation

- **`diff_spef` is a conformance reader, not a correlation harness.** Keep it as the former —
  eight writer defects, none of which our own suite could see, because our own reader never has
  to resolve a reference it did not write. Do not plan a number around it.
- **Eight interop defects are fixed.** Until this exercise, *every* SPEF this engine had ever
  written was unreadable by the incumbent tool chain. That is a larger result than the accuracy
  figure would have been.
- **No R/C correlation number is claimed on this block. Do not quote one from here.**
- **7 % of nets have an RC network in pieces** — the real defect this found, and the one a value
  comparison would have missed.

### Next, in order

1. **Fix `tree::build_network`** so a net's segments form one connected graph. The union-find
   check above is the test: run it over every `*D_NET` and assert one component. That belongs in
   the suite, not in a script — it is checkable entirely from our own output, which is why its
   absence is on us and not on the harness.
2. Re-run this harness. `RCX-0272`/`RCX-0374`/`RCX-0292` going to zero is a real pass/fail gate
   even though no number comes with it.
3. **Then** get a number via OpenRCX `write_spef` + per-net ratio, and extend the deck
   calibration to a **set** of blocks — what `openrcx-counter.md` says is needed and what
   met4/met5 concretely require.

### Worth reporting upstream

Two things, with a reproducer each, and they are related: `diff_spef` is unreachable-by-design at
its own documented output *and* has no test, which is exactly the condition in which the
null-dereference on an unresolved name survived.
