//! Lateral coupling capacitance — geometric adjacency model (toward M5).
//!
//! Coupling capacitance is the hardest, highest-value term in extraction: a
//! wire couples to its neighbours, shifting both delay and signal integrity.
//! Two segments on the same layer, belonging to different nets, that run parallel
//! and overlap, couple with
//!
//! ```text
//!   Cc = coupling_per_um[layer] * visible_length * shape[layer](gap)
//! ```
//!
//! ignored beyond `couple_cutoff`. `coupling_per_um` is the per-micron coupling at
//! the reference spacing `s_ref`. The `gap` is the true **edge-to-edge** spacing when
//! routing widths are known (`gap = centerline_distance - (w_a + w_b)/2`); with no
//! widths it degrades to the centerline distance. `shape` is the layer's characterised
//! fall-off when the deck carries one, else the analytic `s_ref / max(gap, s_ref)` —
//! see [`couple_shape`], and note that real coupling does *not* fall off as 1/s.
//!
//! **`visible_length` is not the whole parallel run.** A wire couples to what it can
//! see: metal between two wires is field that never arrives. Shadowing is resolved run
//! length by run length — a neighbour hides exactly the length it spans, and the
//! remainder still reaches wires further out. The power grid takes part as metal that
//! blocks and never couples, because an AC ground takes field as *grounded* cap. This
//! mirrors the reference extractor's own walk; see [`extract_coupling_full`].
//!
//! A field-solved 2.5-D kernel + golden-pattern fit is the remaining M5 upgrade,
//! replacing the coefficient behind the same SPEF output.
//!
//! **Scaling.** Coupling is inherently local — two wires only couple within
//! `couple_cutoff` — so we never test all net pairs. Every segment is binned into
//! a uniform spatial grid (cell ≈ cutoff), and each segment is compared only with
//! the handful of segments in its own and neighbouring cells. That turns the naive
//! `O(nets² × segments²)` sweep into work that grows with the routed area, not its
//! square, so the engine holds up on full blocks instead of just small cells.
//!
//! *Memory* scales with the number of *distinct* coupling pairs, which on an
//! ultra-dense block can reach the tens of millions. Two things keep it bounded: the
//! accumulator is keyed by net index (16 bytes) rather than by cloned name strings,
//! and a safety valve (`VYGES_MAX_COUPLING_PAIRS`) caps the distinct-pair count so a
//! pathological input degrades gracefully instead of exhausting RAM — see
//! [`extract_coupling`].
//!
//! Pure std — fully unit-tested offline.

use std::collections::{BTreeMap, HashMap};

use rayon::prelude::*;

use crate::def::{DefNet, Segment};
use crate::rules::RcRules;

#[derive(Debug, Clone)]
pub struct CouplingCap {
    pub a: String, // net a
    pub b: String, // net b
    pub cap_ff: f64,
}

/// Overlap area of two axis-aligned rectangles (xmin, ymin, xmax, ymax).
fn rect_overlap(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> f64 {
    let dx = (a.2.min(b.2) - a.0.max(b.0)).max(0.0);
    let dy = (a.3.min(b.3) - a.1.max(b.1)).max(0.0);
    dx * dy
}

/// Lateral coupling capacitance (fF) for a parallel run of `ov` µm at edge-to-edge
/// spacing `gap` µm on `layer` — the physics, with the pairing strategy (which run
/// length is visible at which spacing) decided by the caller. Field kernel when a
/// thickness/eps_r is known, else the rule coefficient scaled by the layer's
/// fall-off shape. Returns 0 when the layer has no coupling rule.
fn lateral_cap(
    layer: &str,
    gap: f64,
    ov: f64,
    rules: &RcRules,
    thickness: &dyn Fn(&str) -> f64,
) -> f64 {
    let Some(l) = rules.layer(layer) else {
        return 0.0;
    };
    let t = thickness(layer);
    if rules.eps_r > 0.0 && t > 0.0 {
        // M5 field kernel: sidewall parallel-plate from real T + gap,
        // fringe-corrected by the ground-competition fall-off when the
        // layer height is known (else bare plate).
        let h = rules.heights.get(layer).copied().unwrap_or(0.0);
        crate::field::coupling_per_um_fringe(rules.eps_r, t, h, gap) * ov
    } else if l.coupling_per_um > 0.0 {
        l.coupling_per_um * ov * couple_shape(rules, layer, l, gap)
    } else {
        0.0
    }
}

/// Inter-layer (crossover) coupling: an areal cap over the two footprints' overlap.
/// Needs widths, so it is zero without a LEF. Nothing lies between the plates, so
/// this term is exempt from the line-of-sight rule that governs lateral coupling.
fn crossover_cap(
    sa: &Segment,
    sb: &Segment,
    rules: &RcRules,
    seg_width: &dyn Fn(&Segment) -> f64,
) -> f64 {
    match rules.interlayer(&sa.layer, &sb.layer) {
        Some(coeff) => coeff * rect_overlap(sa.footprint(seg_width(sa)), sb.footprint(seg_width(sb))),
        None => 0.0,
    }
}

/// The fall-off factor for a layer at edge-to-edge spacing `gap`.
///
/// Prefers the layer's **characterised curve** (`couple_shape` in the deck) and falls back to the
/// analytic `s_ref / max(gap, s_ref)` when none is given.
///
/// The distinction matters more than it looks. Real coupling does not fall off as 1/s: measured
/// from sky130A's reference deck, met1 keeps **0.773** of its minimum-spacing coupling at twice
/// the spacing, where 1/s would give 0.500. A 1/s model fitted to reproduce *totals* therefore
/// runs systematically low per pair — the measured ~20 % — and has to inflate its coefficient to
/// compensate, which is what made that coefficient look unphysical.
///
/// Interpolation follows the reference's own `getComputeRC`: clamp below the first sample, linear
/// between samples, and a 1/gap tail past the last one — the tail being the only place a `1/s`
/// fall-off is actually right.
fn couple_shape(rules: &RcRules, layer: &str, l: &crate::rules::LayerRc, gap: f64) -> f64 {
    let Some(pts) = rules.couple_shapes.get(layer).filter(|p| !p.is_empty()) else {
        // No characterised curve: the original analytic shape.
        return l.s_ref / gap.max(l.s_ref);
    };
    let (s0, f0) = pts[0];
    if gap <= s0 {
        return f0; // clamp — the curve is not characterised below its first sample
    }
    for w in pts.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if gap >= x0 && gap < x1 {
            let span = x1 - x0;
            return if span <= 0.0 { y0 } else { y0 + (y1 - y0) * (gap - x0) / span };
        }
    }
    // Past the table: 1/gap from the last characterised point.
    let (sn, fnl) = pts[pts.len() - 1];
    fnl * sn / gap
}

/// Coupling caps between every pair of nets — one aggregated entry per net pair.
///
/// `widths` maps layer → default routing width (um, from LEF). When present the
/// gap is edge-to-edge; when a layer is absent (width 0) it degrades to the
/// centerline distance — so an empty map reproduces the pre-LEF behaviour.
///
/// Only spatially-near segment pairs are tested, via a uniform grid (see the
/// module header) — the result is identical to the exhaustive sweep but the cost
/// scales with routed area, not net-count squared.
///
/// **Memory bound.** The accumulator is keyed by net *index* (`(u32, u32)`), not by
/// cloned net-name strings, so a distinct coupling pair costs 16 bytes of key rather
/// than two heap-allocated `String`s — roughly an order of magnitude less on the
/// dense blocks where the pair count runs into the tens of millions. A safety valve
/// (`VYGES_MAX_COUPLING_PAIRS`, default [`DEFAULT_MAX_COUPLING_PAIRS`]) caps the number
/// of *distinct* pairs held at once: past the cap, contributions to already-seen pairs
/// still accumulate but genuinely new pairs are dropped with a one-line warning, so a
/// pathological ultra-dense input degrades gracefully instead of exhausting RAM.
pub fn extract_coupling(
    nets: &[DefNet],
    rules: &RcRules,
    widths: &BTreeMap<String, f64>,
    thicknesses: &BTreeMap<String, f64>,
) -> Vec<CouplingCap> {
    extract_coupling_full(nets, rules, widths, thicknesses, &[], max_coupling_pairs())
}

/// As [`extract_coupling`], with the power grid supplied as `blockers`.
///
/// Power wires never appear in the output — an AC ground takes field as *grounded*
/// capacitance, not as coupling, which is why the reference SPEF contains no power
/// nets at all. But they are metal, and metal blocks: the reference walks outward
/// from a wire and a power rail ends that walk (`w2->isPower()`), so two signals on
/// either side of a rail do not couple through it. On sky130 standard-cell rows a
/// met1 rail runs through every row, so leaving them out is not a rounding error.
pub fn extract_coupling_blocked(
    nets: &[DefNet],
    rules: &RcRules,
    widths: &BTreeMap<String, f64>,
    thicknesses: &BTreeMap<String, f64>,
    blockers: &[Segment],
) -> Vec<CouplingCap> {
    extract_coupling_full(
        nets,
        rules,
        widths,
        thicknesses,
        blockers,
        max_coupling_pairs(),
    )
}

/// Distinct-pair cap for the coupling accumulator when none is set via
/// `VYGES_MAX_COUPLING_PAIRS`. 100M pairs ≈ a few GB with the integer-keyed map — far
/// above what any real routed block produces (a 10k-net block couples into low
/// single-digit millions of pairs), so this only ever bites on pathological input.
pub const DEFAULT_MAX_COUPLING_PAIRS: usize = 100_000_000;

/// Distinct-pair cap from `VYGES_MAX_COUPLING_PAIRS`, else [`DEFAULT_MAX_COUPLING_PAIRS`].
fn max_coupling_pairs() -> usize {
    std::env::var("VYGES_MAX_COUPLING_PAIRS")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_COUPLING_PAIRS)
}

/// As [`extract_coupling`], but with an explicit cap on the number of distinct net
/// pairs held in the accumulator (see the memory-bound note there). Exposed for tests
/// that exercise the safety valve without touching process-wide env.
pub fn extract_coupling_capped(
    nets: &[DefNet],
    rules: &RcRules,
    widths: &BTreeMap<String, f64>,
    thicknesses: &BTreeMap<String, f64>,
    max_pairs: usize,
) -> Vec<CouplingCap> {
    extract_coupling_full(nets, rules, widths, thicknesses, &[], max_pairs)
}

/// Net index reserved for the power grid: metal that occludes but is never reported
/// as a coupling partner.
const POWER_NET: usize = usize::MAX;

/// The general form behind [`extract_coupling`], [`extract_coupling_blocked`] and
/// [`extract_coupling_capped`] — every knob explicit.
pub fn extract_coupling_full(
    nets: &[DefNet],
    rules: &RcRules,
    widths: &BTreeMap<String, f64>,
    thicknesses: &BTreeMap<String, f64>,
    blockers: &[Segment],
    max_pairs: usize,
) -> Vec<CouplingCap> {
    let width = |layer: &str| widths.get(layer).copied().unwrap_or(0.0);
    let thickness = |layer: &str| thicknesses.get(layer).copied().unwrap_or(0.0);

    // flatten to a global segment list tagged with its owning net
    struct SegRef<'a> {
        net: usize,
        seg: &'a Segment,
    }
    let mut segs: Vec<SegRef> = Vec::new();
    for (ni, n) in nets.iter().enumerate() {
        for s in &n.segments {
            segs.push(SegRef { net: ni, seg: s });
        }
    }
    // Power wires ride the same index space so the spatial grid and the line-of-sight
    // walk see them, but carry a net id that can never be emitted.
    for s in blockers {
        segs.push(SegRef {
            net: POWER_NET,
            seg: s,
        });
    }

    // A segment's own drawn width when it has one (an NDR route, or a PDN strap, which is
    // an order of magnitude wider than the layer default) — else the layer's routing width.
    let seg_width = |sg: &Segment| {
        if sg.width_um > 0.0 {
            sg.width_um
        } else {
            width(&sg.layer)
        }
    };

    // Cell size ≈ the interaction range: the cutoff plus the widest wire, so any
    // two segments that could couple land in the same or an adjacent cell. Floored
    // to keep the grid from exploding when the cutoff is tiny. Power straps count
    // toward "widest": a 1.6 µm strap binned as a 0.14 µm wire would go unseen.
    let maxw = widths
        .values()
        .copied()
        .chain(blockers.iter().map(|s| s.width_um))
        .fold(0.0_f64, f64::max);
    let cell = (rules.couple_cutoff + maxw).max(MIN_CELL_UM);
    let pad = rules.couple_cutoff + maxw; // query halo so near pairs are never missed
    let bin = |v: f64| (v / cell).floor() as i64;

    // bin each segment's footprint cells (layer-independent grid; the layer check
    // lives in the walk, and crossover coupling needs cross-layer neighbours)
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let bbox = |sr: &SegRef| sr.seg.footprint(seg_width(sr.seg));
    for (id, sr) in segs.iter().enumerate() {
        let (xlo, ylo, xhi, yhi) = bbox(sr);
        for xb in bin(xlo)..=bin(xhi) {
            for yb in bin(ylo)..=bin(yhi) {
                grid.entry((xb, yb)).or_default().push(id);
            }
        }
    }

    // ── line of sight, by coverage ───────────────────────────────────────────────────
    // A wire couples to what it can SEE, and what hides it is decided **run-length by
    // run-length**, not wire by wire.
    //
    // The reference (OpenRCX v1 `couplingFlow`, which is what `extract_parasitics` runs
    // without `-version`) carries each source wire as a set of *uncovered* pieces and
    // walks outward one track at a time — `Track::findOverlap` in `extCC.cpp`. Against
    // each neighbour it splits the piece into (before, overlapping, after): the
    // overlapping part is booked as coupling to that neighbour and is CONSUMED, while
    // the before/after remainders stay uncovered and are carried to the next track out.
    // The walk stops when nothing uncovered is left.
    //
    // So a short neighbour occludes only the length it actually spans. Our first cut at
    // this took the nearest neighbour per side and stopped, which over-pruned: the fit
    // then had to raise the coupling coefficient to put back what was removed (met1
    // 0.88× → 1.80× of the reference's own characterised value). Shadowing by coverage
    // is the mechanism, and it is what this does.
    //
    // Direction: the reference books a pair once, from the lower wire looking up (the
    // downward pass runs with `handleEmptyOnly`, which suppresses coupling and keeps
    // only the ground bookkeeping). We do the same — walk one side, so each pair is
    // emitted exactly once, by the segment below it.
    //
    // Only strictly axis-parallel segments take part; a via footprint has no run to
    // couple over.
    let horiz = |sg: &Segment| sg.is_horizontal();
    let axial = |sg: &Segment| sg.is_horizontal() || sg.is_vertical();
    let perp_centre = |sg: &Segment| {
        if horiz(sg) {
            (sg.y0 + sg.y1) / 2.0
        } else {
            (sg.x0 + sg.x1) / 2.0
        }
    };
    let run_span = |sg: &Segment| {
        if horiz(sg) {
            (sg.x0.min(sg.x1), sg.x0.max(sg.x1))
        } else {
            (sg.y0.min(sg.y1), sg.y0.max(sg.y1))
        }
    };

    // Per segment: the same-layer neighbours it can still see on its "up" side, each with
    // the gap and the run length that reaches it. Empty for a power wire (metal that
    // blocks but never couples) and for anything non-axial.
    let visible: Vec<Vec<(usize, f64, f64)>> = (0..segs.len())
        .into_par_iter()
        .map(|id| {
            let sr = &segs[id];
            let sg = sr.seg;
            if sr.net == POWER_NET || !axial(sg) {
                return Vec::new();
            }
            let (a0, a1) = run_span(sg);
            if a1 <= a0 {
                return Vec::new();
            }
            let (xlo, ylo, xhi, yhi) = bbox(sr);
            let mut cand: Vec<usize> = Vec::new();
            for xb in bin(xlo - pad)..=bin(xhi + pad) {
                for yb in bin(ylo - pad)..=bin(yhi + pad) {
                    if let Some(ids) = grid.get(&(xb, yb)) {
                        cand.extend(ids.iter().copied());
                    }
                }
            }
            cand.sort_unstable();
            cand.dedup();

            let wa = seg_width(sg);
            let ca = perp_centre(sg);
            // Everything on the up side that is close enough to matter, ordered by
            // centre-line distance — the walk's track order.
            let mut reach: Vec<(f64, f64, f64, f64, usize)> = Vec::new(); // (delta, b0, b1, gap, id)
            for o in cand {
                if o == id {
                    continue;
                }
                let og = segs[o].seg;
                // Only same-layer, same-orientation metal occludes or couples laterally.
                if og.layer != sg.layer || !axial(og) || horiz(og) != horiz(sg) {
                    continue;
                }
                let delta = perp_centre(og) - ca;
                if delta <= 0.0 {
                    continue; // the other side's walk owns it
                }
                let (b0, b1) = run_span(og);
                if b1 <= a0 || a1 <= b0 {
                    continue; // no parallel run
                }
                let gap = delta - (wa + seg_width(og)) / 2.0;
                // Past the cutoff a wire neither couples nor blocks, exactly as the
                // reference's `inThreshold` test leaves the piece uncovered.
                if gap > rules.couple_cutoff {
                    continue;
                }
                reach.push((delta, b0, b1, gap, o));
            }
            // Ties (same track) cannot overlap in run, so any stable order does; sort on
            // the full key anyway so the result never depends on grid iteration order.
            reach.sort_unstable_by(|x, y| {
                x.0.total_cmp(&y.0)
                    .then_with(|| x.1.total_cmp(&y.1))
                    .then_with(|| x.4.cmp(&y.4))
            });

            let mut uncovered: Vec<(f64, f64)> = vec![(a0, a1)];
            let mut out: Vec<(usize, f64, f64)> = Vec::new();
            for (_, b0, b1, gap, o) in reach {
                let seen = visible_len(&uncovered, b0, b1);
                if seen > 0.0 {
                    // Same-net metal blocks just as well as anyone else's — it is metal in
                    // the way — but a net does not couple to itself.
                    if segs[o].net != sr.net && segs[o].net != POWER_NET {
                        out.push((o, gap, seen));
                    }
                    occlude(&mut uncovered, b0, b1);
                    if uncovered.is_empty() {
                        break;
                    }
                }
            }
            out
        })
        .collect();

    // Accumulate by net *index* (u32), not cloned name strings — an order of magnitude
    // less memory per distinct pair, and no String allocation on the hot path.
    //
    // **Band-partitioned parallelism.** Every unordered segment pair `(id, other)` with
    // `other > id` is owned by exactly the worker whose *contiguous* outer-`id` band
    // contains `id`, so bands never re-test the same pair and their key sets are largely
    // disjoint — total memory across all band maps stays ≈ one serial map, not
    // `threads ×` it (the trap that OOM'd the naive per-block prototype). Because the
    // bands are contiguous and ascending and we merge them in band order, every net
    // pair's contributions are summed in the exact same left-to-right order as the
    // serial sweep, so the result is **bit-identical** to `-j1` — not merely close.
    //
    // Scan one outer-`id` band into its own map (no cap here: the global cap is applied
    // once, at merge, so it bounds the true distinct-pair total rather than per band).
    let scan_band = |lo: usize, hi: usize| -> HashMap<(u32, u32), f64> {
        let mut acc: HashMap<(u32, u32), f64> = HashMap::new();
        let add = |a: usize, b: usize, cc: f64, acc: &mut HashMap<(u32, u32), f64>| {
            if cc > 0.0 {
                let (ni, nj) = (a.min(b), a.max(b));
                *acc.entry((ni as u32, nj as u32)).or_default() += cc;
            }
        };
        for id in lo..hi {
            let sr = &segs[id];
            if sr.net == POWER_NET {
                continue; // the grid takes field as ground, never as a coupling partner
            }
            // Lateral: exactly the run each neighbour can still see, at its own gap.
            for &(other, gap, ov) in &visible[id] {
                let cc = lateral_cap(&sr.seg.layer, gap, ov, rules, &thickness);
                add(sr.net, segs[other].net, cc, &mut acc);
            }
            // Crossover: an overlap term with nothing between the plates, so it is exempt
            // from line of sight and still needs the neighbour sweep.
            if rules.interlayer.is_empty() {
                continue;
            }
            let (xlo, ylo, xhi, yhi) = bbox(sr);
            // gather candidate segment ids from the cells the padded footprint touches
            let mut cand: Vec<usize> = Vec::new();
            for xb in bin(xlo - pad)..=bin(xhi + pad) {
                for yb in bin(ylo - pad)..=bin(yhi + pad) {
                    if let Some(ids) = grid.get(&(xb, yb)) {
                        cand.extend(ids.iter().copied());
                    }
                }
            }
            cand.sort_unstable();
            cand.dedup();
            for other in cand {
                // each unordered pair once (other > id); never self-couple a net
                if other <= id
                    || segs[other].net == sr.net
                    || segs[other].net == POWER_NET
                    || sr.seg.layer == segs[other].seg.layer
                {
                    continue;
                }
                let cc = crossover_cap(sr.seg, segs[other].seg, rules, &seg_width);
                add(sr.net, segs[other].net, cc, &mut acc);
            }
        }
        acc
    };

    // Contiguous outer-`id` bands. A few bands per thread give rayon room to steal when
    // some regions are denser than others, without materially inflating hot-key
    // replication (only pairs spanning many bands replicate, and those are few).
    let threads = rayon::current_num_threads().max(1);
    let nbands = (threads * 4).min(segs.len().max(1));
    let band = segs.len().div_ceil(nbands.max(1)).max(1);
    let bands: Vec<(usize, usize)> = (0..segs.len())
        .step_by(band)
        .map(|lo| (lo, (lo + band).min(segs.len())))
        .collect();
    // `collect()` preserves band order, so the merge below is deterministic.
    let partials: Vec<HashMap<(u32, u32), f64>> = bands
        .par_iter()
        .map(|&(lo, hi)| scan_band(lo, hi))
        .collect();

    // Merge bands in order. Same-key adds happen band-by-band (= ascending id order),
    // so the summed cap of each pair is bit-identical to the serial sweep. The safety
    // valve applies here, to the true distinct-pair total.
    let mut acc: HashMap<(u32, u32), f64> = HashMap::new();
    let mut dropped: u64 = 0; // distinct pairs refused once the cap is hit
    for pm in partials {
        for (key, v) in pm {
            if let Some(g) = acc.get_mut(&key) {
                *g += v;
            } else if acc.len() < max_pairs {
                acc.insert(key, v);
            } else {
                dropped += 1;
            }
        }
    }
    if dropped > 0 {
        eprintln!(
            "warning: vyges-extract coupling: distinct net-pair count exceeded the cap of \
             {max_pairs}; {dropped} further pair(s) dropped to bound memory. Tighten \
             `couple_cutoff` or raise VYGES_MAX_COUPLING_PAIRS to extract them."
        );
    }
    // Resolve indices → names and sort by (a, b) so output is identical (byte-for-byte
    // in the rendered SPEF) to the previous name-keyed BTreeMap iteration order.
    let mut out: Vec<CouplingCap> = acc
        .into_iter()
        .filter(|(_, c)| *c > 0.0)
        .map(|((ni, nj), cap_ff)| CouplingCap {
            a: nets[ni as usize].name.clone(),
            b: nets[nj as usize].name.clone(),
            cap_ff,
        })
        .collect();
    out.sort_unstable_by(|x, y| x.a.cmp(&y.a).then_with(|| x.b.cmp(&y.b)));
    out
}

/// How much of `[b0, b1)` is still uncovered — the run length over which a neighbour
/// spanning that interval is actually in view. `uncovered` is sorted and disjoint.
fn visible_len(uncovered: &[(f64, f64)], b0: f64, b1: f64) -> f64 {
    uncovered
        .iter()
        .map(|&(u0, u1)| (u1.min(b1) - u0.max(b0)).max(0.0))
        .sum()
}

/// Remove `[b0, b1)` from the uncovered set: a neighbour hides the run it spans and
/// only that run, leaving the remainders visible to wires further out. Order and
/// disjointness are preserved.
fn occlude(uncovered: &mut Vec<(f64, f64)>, b0: f64, b1: f64) {
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(uncovered.len() + 1);
    for &(u0, u1) in uncovered.iter() {
        if b1 <= u0 || b0 >= u1 {
            out.push((u0, u1)); // untouched
            continue;
        }
        if u0 < b0 {
            out.push((u0, b0));
        }
        if b1 < u1 {
            out.push((b1, u1));
        }
    }
    *uncovered = out;
}

/// Lower bound on grid cell size (um) — stops a near-zero cutoff from producing an
/// enormous number of tiny cells.
const MIN_CELL_UM: f64 = 1.0;

#[cfg(test)]
mod coverage_tests {
    use super::*;

    /// A blocker in the middle leaves both ends visible — the len1/len3 split.
    #[test]
    fn a_middle_blocker_leaves_two_remainders() {
        let mut un = vec![(0.0, 10.0)];
        assert!((visible_len(&un, 4.0, 6.0) - 2.0).abs() < 1e-12);
        occlude(&mut un, 4.0, 6.0);
        assert_eq!(un, vec![(0.0, 4.0), (6.0, 10.0)]);
        // A wire further out sees only what is left of the run.
        assert!((visible_len(&un, 0.0, 10.0) - 8.0).abs() < 1e-12);
    }

    /// A blocker spanning past both ends leaves nothing — the walk terminates.
    #[test]
    fn a_spanning_blocker_leaves_nothing() {
        let mut un = vec![(0.0, 10.0)];
        occlude(&mut un, -1.0, 11.0);
        assert!(un.is_empty());
        assert_eq!(visible_len(&un, 0.0, 10.0), 0.0);
    }

    /// Blockers that miss the run entirely change nothing.
    #[test]
    fn a_disjoint_blocker_is_a_no_op() {
        let mut un = vec![(0.0, 10.0)];
        assert_eq!(visible_len(&un, 20.0, 30.0), 0.0);
        occlude(&mut un, 20.0, 30.0);
        assert_eq!(un, vec![(0.0, 10.0)]);
    }

    /// Coverage accumulates across several partial blockers without double-counting.
    #[test]
    fn successive_blockers_accumulate() {
        let mut un = vec![(0.0, 10.0)];
        occlude(&mut un, 0.0, 3.0);
        occlude(&mut un, 2.0, 5.0); // overlaps the first — the shared part is not re-counted
        assert_eq!(un, vec![(5.0, 10.0)]);
        assert!((visible_len(&un, 0.0, 10.0) - 5.0).abs() < 1e-12);
    }
}

#[cfg(test)]
mod shape_tests {
    use super::*;
    use crate::rules::LayerRc;

    fn rules_with(shape: Option<Vec<(f64, f64)>>) -> RcRules {
        let mut r = RcRules::parse("met1 0.125 0.078 0.1 0.14\ncouple_cutoff 5.0\n").unwrap();
        if let Some(pts) = shape {
            r.couple_shapes.insert("met1".into(), pts);
        }
        r
    }
    fn met1(r: &RcRules) -> LayerRc {
        *r.layers.get("met1").unwrap()
    }

    /// With no characterised curve the model keeps its original analytic fall-off, so a deck
    /// written before `couple_shape` existed behaves exactly as it did.
    #[test]
    fn falls_back_to_the_analytic_shape() {
        let r = rules_with(None);
        let l = met1(&r);
        assert!((couple_shape(&r, "met1", &l, 0.14) - 1.0).abs() < 1e-12, "at s_ref");
        assert!((couple_shape(&r, "met1", &l, 0.28) - 0.5).abs() < 1e-12, "1/s at twice s_ref");
        assert!((couple_shape(&r, "met1", &l, 0.05) - 1.0).abs() < 1e-12, "clamped below s_ref");
    }

    /// The characterised curve is used where present — and it is nothing like 1/s. sky130A's
    /// met1 keeps 0.773 at twice the minimum spacing where the analytic model gives 0.500;
    /// that gap is the ~20 % per-pair deficit this replaced.
    #[test]
    fn the_characterised_curve_decays_far_slower_than_one_over_s() {
        let r = rules_with(Some(vec![(0.14, 1.0), (0.28, 0.7734), (0.42, 0.6001)]));
        let l = met1(&r);
        assert!((couple_shape(&r, "met1", &l, 0.28) - 0.7734).abs() < 1e-9);
        let analytic = l.s_ref / 0.28_f64.max(l.s_ref);
        assert!(analytic < 0.51, "the analytic model gives {}", analytic);
    }

    /// Between samples the curve is linear, matching the reference's own `getComputeRC`.
    #[test]
    fn interpolates_linearly_between_samples() {
        let r = rules_with(Some(vec![(0.14, 1.0), (0.28, 0.8), (0.42, 0.6)]));
        let l = met1(&r);
        assert!((couple_shape(&r, "met1", &l, 0.21) - 0.9).abs() < 1e-9, "midpoint of 1.0..0.8");
        assert!((couple_shape(&r, "met1", &l, 0.35) - 0.7).abs() < 1e-9, "midpoint of 0.8..0.6");
    }

    /// Below the first sample the curve is clamped: it is not characterised there, and
    /// extrapolating a steepening curve toward zero spacing invents coupling.
    #[test]
    fn clamps_below_the_first_sample() {
        let r = rules_with(Some(vec![(0.14, 1.0), (0.28, 0.7734)]));
        let l = met1(&r);
        assert!((couple_shape(&r, "met1", &l, 0.10) - 1.0).abs() < 1e-12);
        assert!((couple_shape(&r, "met1", &l, 0.0) - 1.0).abs() < 1e-12);
    }

    /// Past the last sample it falls as 1/gap from that point — the one regime where a 1/s
    /// fall-off is the right shape, and what the reference does there too.
    #[test]
    fn tails_off_as_one_over_gap_past_the_table() {
        let r = rules_with(Some(vec![(0.14, 1.0), (1.4, 0.25)]));
        let l = met1(&r);
        assert!((couple_shape(&r, "met1", &l, 2.8) - 0.25 * 1.4 / 2.8).abs() < 1e-9);
        assert!(couple_shape(&r, "met1", &l, 100.0) < 0.005, "decays away");
    }

    /// A deck round-trips the curve, so a fitted deck can be written back out.
    #[test]
    fn the_deck_parses_a_shape_line() {
        let r = RcRules::parse(
            "met1 0.125 0.078 0.1 0.14\ncouple_shape met1 0.14:1.0 0.28:0.7734 0.42:0.6001\n",
        )
        .unwrap();
        let pts = r.couple_shapes.get("met1").expect("parsed");
        assert_eq!(pts.len(), 3);
        assert!((pts[1].1 - 0.7734).abs() < 1e-12);
    }

    /// Points given out of order are sorted, so a hand-edited deck cannot silently produce a
    /// non-monotonic curve that the interpolator would read as a step.
    #[test]
    fn shape_points_are_sorted_by_spacing() {
        let r = RcRules::parse(
            "met1 0.125 0.078 0.1 0.14\ncouple_shape met1 0.42:0.6 0.14:1.0 0.28:0.77\n",
        )
        .unwrap();
        let pts = r.couple_shapes.get("met1").unwrap();
        assert_eq!(pts[0].0, 0.14);
        assert_eq!(pts[2].0, 0.42);
    }
}
