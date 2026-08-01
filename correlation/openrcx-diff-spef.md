# Correlating against OpenRCX with `diff_spef` — the harness, and what it found first

- **Written:** 2026-08-01
- **Status:** harness built and running; **the numeric correlation is not yet obtained** — see
  the blocker at the end. Four interop findings came out before it, three of them fixed.
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

## ⚠️ What it says about our RC, which is the real work

The reader is satisfied; the *contents* are not yet. On this block OpenRCX reports:

| | count | of 14 286 nets |
| --- | ---: | --- |
| `RCX-0272` RC **disconnected** | **1 001** | 7.0 % |
| `RCX-0374` RC **inconsistency** | **628** | 4.4 % |
| `RCX-0292` **looped** spef RC | 23 | 0.2 % |
| `RCX-0044` spef inst not in db | 1 | — |

`Unmatched spef and db` still stops the numeric report, so **there is still no R/C correlation
figure** — but this is now a diagnosis of our RC *topology* rather than of our file format, which
is a different and more interesting problem. A net whose RC network OpenRCX cannot walk as a
connected tree is one whose parasitics no downstream timer can use properly either.

That is the next piece of work, and it is extraction work rather than formatting work.

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

- The **harness is real and reusable**; the Tcl above is the whole of it.
- **Three interop defects are fixed**, and they were serious: until today every SPEF this engine
  produced was unreadable by the incumbent tool chain. That is a bigger result than the accuracy
  number would have been.
- **No R/C correlation number is claimed** on this block. Do not quote one from here.

### Next, in order

1. **A small routed sky130 block with a loadable DEF/LEF** — unblocks isolation of the segfault
   and gives every future OpenROAD comparison a cheap vehicle.
2. Re-run `diff_spef` there; if it still crashes on our file, bisect the SPEF to the construct
   that triggers it and report upstream.
3. Only then extend the deck calibration to a **set** of blocks, which is what
   `openrcx-counter.md` says is needed and what met4/met5 now concretely require.
