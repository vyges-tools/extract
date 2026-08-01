# Correlating against OpenRCX with `diff_spef` — the harness, and what it found first

- **Written:** 2026-08-01
- **Status:** harness built; OpenRCX now **reads our SPEF completely**, which it could not do at
  all before. **No numeric correlation is obtainable this way** — `diff_spef.out` is unreachable
  in OpenROAD as pinned, verified in source. What the exercise *did* produce: **eight** writer
  defects, all fixed, and — via its topology checks — **four extraction/reader defects that made
  7 % of nets emit an RC network in more than one piece**, now all at zero. Accuracy against the
  sign-off SPEF is measured separately in [`ground-vs-coupling.md`](ground-vs-coupling.md).
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

## ✅ What the reader said about our RC, and what fixing it took

`diff_spef` cannot give values, but its topology checks run, and they found something real.
Four defects, in three different files, each only visible because the incumbent read our output:

| | at the start | after |
| --- | ---: | ---: |
| `RCX-0272` RC **disconnected** | **1 001** (7.0 %) | **0** |
| `RCX-0374` RC **inconsistency** | 630 (4.4 %) | **0** |
| `RCX-0292` **looped** spef RC | 23 | **0** |

### 1. Wires that meet in the middle were never joined

All 1 001 disconnected nets were internal multi-pin nets. Union-find over `_00768_`'s own
`*RES` edges (4 pins, 19 nodes, 16 resistors) gave **three components** — the driver stranded
with one sink, two sinks in another graph, four internal nodes reachable from nothing. A timer
reading that sees no interconnect delay to those pins at all.

The tree builder interned a node at each segment **endpoint**. Real routing constantly joins a
wire at a point that is an endpoint of one and the *interior* of the other:

- **a via lands mid-span** — `NEW met1 ( 230690 1040230 ) M1M2_PR` connects to a met2 run from
  y=1035470 to y=1043460, 4.76 µm along it, an endpoint of nothing;
- **a same-layer T-junction**, where a branch taps a spine between its ends.

Segments are now split at those junctions, in integer nanometres so "do these meet" is an exact
comparison. Sub-lengths sum to the original and both cap-per-µm and `wire_res` are linear in
length, so **`raw_cap`/`raw_res` are unchanged and the deck calibration is untouched** — verified
on the block, where the extracted total did not move by a femtofarad.

Doing that needed a DEF-reader change: vias were kept as a **count**, and the location is the
whole point (`vyges-tools/loom 07aa87a`).

### 2. …and then the fix invented connectivity, which was worse

Loops went **23 → 70**. Splitting puts nodes wherever *another* layer's via cut through, and the
via pass chained every layer present at a location — so a met2→met3 via at a point the met1 wire
merely ran past also shorted met1 to met2, and where those two were legitimately joined further
along, the net came back with a loop in it.

The first fix for that was also wrong: "a via is declared on the lower of the two layers it
joins" is true of this block and **false in general** — `examples/counter` writes it the other
way round, and DEF permits both. Only the **LEF `VIA` block** says which two layers a via joins,
so that is the authority now (`vyges-tools/loom 21cfe0b`, which also stops a VIA block's own
`LAYER` lines being read as tech layers). Without a LEF, the declared layer plus the one adjacent
layer present settles every two-layer case; where a layer sits both above and below, the question
is genuinely open and **we decline** — the net degrades to a star, counted and visible, rather
than being wired up wrong and looking fine. **70 → 11.**

### 3. `RECT` drew a wire to the origin

All 11 remaining looped nets contained a `RECT`. `RECT ( 0 -150 390 150 )` is a via-landing patch
stated as an **offset rectangle** from the preceding point. The reader skipped the keyword but not
its body, so the next group was read as a coordinate — drawing a wire from the routing point to
(0.000, -0.150). Two of those in one net meet down there and tie distant parts of it together.
**791 of them in this block** (`vyges-tools/loom 50540e6`). **11 → 0.**

That one moved the numbers: **the block's extracted capacitance fell 35 %** (294 837 → 190 991 fF),
because those phantom wires were ~1 mm long. Any figure previously taken from a block containing
`RECT` was inflated. The calibration block has none, so [`openrcx-counter.md`](openrcx-counter.md)
stands.

### A backstop, so the invariant holds unconditionally

If a network still will not resolve into one piece, the builder says so and the caller emits the
lumped star, which is connected by construction — coarse and valid beats detailed and invalid. It
is counted and reported as `EXTRACT-RC-DISCONNECTED`, not folded into the same silence as "this
net has no routing". On this block it fires zero times.

## ➡️ The accuracy question, now answered — see the companion doc

With the topology right, the magnitudes are finally worth measuring, and the decomposition lives
in **[`ground-vs-coupling.md`](ground-vs-coupling.md)**. In short: **ground 1.37× over,
coupling 0.33× (3× under)** — two separable errors in opposite directions that had been partially
cancelling. Chasing the coupling half found a LEF reader bug that reported met1's routing width as
3 µm.

⚠️ An earlier draft of this file quoted **2.83×** for the total. That was wrong: it compared their
`*D_NET` sum (coupling counted once) against ours (counted on both nets). On a like-for-like rule
the total gap is **1.81×**. The companion doc has the arithmetic.

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

1. **Fit the coupling coefficient**, which [`ground-vs-coupling.md`](ground-vs-coupling.md)
   shows is 74 % of the error and has never been calibrated against anything. Then the ground
   term's additive component, then the deck across a set of blocks.
2. Keep `diff_spef` as the **topology gate**. `RCX-0272`/`RCX-0374`/`RCX-0292` at zero is a real
   pass/fail even though no value comes with it, and it is cheap to re-run.
3. Port the union-find check into the suite as a per-`*D_NET` assertion so a regression is caught
   without needing OpenROAD at all. `tree::components` is already the shared implementation.

### Worth reporting upstream

`diff_spef` is unreachable-by-design at its own documented output *and* has no test — which is
exactly the condition in which the null-dereference on an unresolved name survived. Reproducer in
hand for both.
