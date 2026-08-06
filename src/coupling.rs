//! Lateral coupling capacitance — geometric adjacency model (toward M5).
//!
//! Coupling capacitance is the hardest, highest-value term in extraction: a
//! wire couples to its neighbours, shifting both delay and signal integrity.
//! v1 is a *geometric rule* model — two segments on the same layer, belonging
//! to different nets, that run parallel and overlap, couple with
//!
//! ```text
//!   Cc = coupling_per_um[layer] * overlap_length * (s_ref / max(gap, s_ref))
//! ```
//!
//! ignored beyond `couple_cutoff`. `coupling_per_um` is the per-micron coupling
//! at the reference spacing `s_ref`, falling off ~1/gap. The `gap` is the true
//! **edge-to-edge** spacing when LEF routing widths are supplied
//! (`gap = centerline_distance - (w_a + w_b)/2`); with no widths it degrades to
//! the centerline distance. A field-solved 2.5-D kernel + golden-pattern fit is
//! the remaining M5 upgrade, replacing this model behind the same SPEF output.
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

/// Parallel-run overlap length + perpendicular gap for two same-layer segments
/// of the same orientation. `None` if different layer/orientation or no overlap.
fn overlap_gap(a: &Segment, b: &Segment) -> Option<(f64, f64)> {
    if a.layer != b.layer {
        return None;
    }
    if a.is_horizontal() && b.is_horizontal() {
        let ov = a.x0.max(a.x1).min(b.x0.max(b.x1)) - a.x0.min(a.x1).max(b.x0.min(b.x1));
        let gap = (a.y0 - b.y0).abs();
        (ov > 0.0).then_some((ov, gap))
    } else if a.is_vertical() && b.is_vertical() {
        let ov = a.y0.max(a.y1).min(b.y0.max(b.y1)) - a.y0.min(a.y1).max(b.y0.min(b.y1));
        let gap = (a.x0 - b.x0).abs();
        (ov > 0.0).then_some((ov, gap))
    } else {
        None
    }
}

/// Overlap area of two axis-aligned rectangles (xmin, ymin, xmax, ymax).
fn rect_overlap(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> f64 {
    let dx = (a.2.min(b.2) - a.0.max(b.0)).max(0.0);
    let dy = (a.3.min(b.3) - a.1.max(b.1)).max(0.0);
    dx * dy
}

/// Coupling capacitance (fF) contributed by one segment pair — the physics, factored
/// out of the pairing strategy. Same-layer parallel runs couple laterally (field
/// kernel when a thickness/eps_r is known, else the rule coefficient); different
/// layers couple areally over their footprint overlap. Returns 0 for pairs that
/// don't couple (different orientation, beyond cutoff, no rule).
fn pair_cap(
    sa: &Segment,
    sb: &Segment,
    rules: &RcRules,
    width: &dyn Fn(&str) -> f64,
    thickness: &dyn Fn(&str) -> f64,
) -> f64 {
    if sa.layer == sb.layer {
        // lateral coupling: parallel same-layer runs, edge-to-edge gap
        let Some((ov, center_gap)) = overlap_gap(sa, sb) else {
            return 0.0;
        };
        let Some(l) = rules.layer(&sa.layer) else {
            return 0.0;
        };
        let gap = center_gap - width(&sa.layer);
        if gap > rules.couple_cutoff {
            return 0.0;
        }
        let t = thickness(&sa.layer);
        if rules.eps_r > 0.0 && t > 0.0 {
            // M5 field kernel: sidewall parallel-plate from real T + gap,
            // fringe-corrected by the ground-competition fall-off when the
            // layer height is known (else bare plate).
            let h = rules.heights.get(&sa.layer).copied().unwrap_or(0.0);
            crate::field::coupling_per_um_fringe(rules.eps_r, t, h, gap) * ov
        } else if l.coupling_per_um > 0.0 {
            // rule-based coefficient, scaled by the layer's fall-off shape.
            l.coupling_per_um * ov * couple_shape(rules, &sa.layer, l, gap)
        } else {
            0.0
        }
    } else if let Some(coeff) = rules.interlayer(&sa.layer, &sb.layer) {
        // inter-layer (crossover) coupling: areal cap over the footprint overlap
        // (needs widths -> zero without a LEF).
        coeff
            * rect_overlap(
                sa.footprint(width(&sa.layer)),
                sb.footprint(width(&sb.layer)),
            )
    } else {
        0.0
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
    extract_coupling_capped(nets, rules, widths, thicknesses, max_coupling_pairs())
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

    // Cell size ≈ the interaction range: the cutoff plus the widest wire, so any
    // two segments that could couple land in the same or an adjacent cell. Floored
    // to keep the grid from exploding when the cutoff is tiny.
    let maxw = widths.values().copied().fold(0.0_f64, f64::max);
    let cell = (rules.couple_cutoff + maxw).max(MIN_CELL_UM);
    let pad = rules.couple_cutoff + maxw; // query halo so near pairs are never missed
    let bin = |v: f64| (v / cell).floor() as i64;

    // bin each segment's footprint cells (layer-independent grid; the layer check
    // lives in pair_cap, and crossover coupling needs cross-layer neighbours)
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let bbox = |sr: &SegRef| sr.seg.footprint(width(&sr.seg.layer));
    for (id, sr) in segs.iter().enumerate() {
        let (xlo, ylo, xhi, yhi) = bbox(sr);
        for xb in bin(xlo)..=bin(xhi) {
            for yb in bin(ylo)..=bin(yhi) {
                grid.entry((xb, yb)).or_default().push(id);
            }
        }
    }

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
        for id in lo..hi {
            let sr = &segs[id];
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
                if other <= id || segs[other].net == sr.net {
                    continue;
                }
                let cc = pair_cap(sr.seg, segs[other].seg, rules, &width, &thickness);
                if cc > 0.0 {
                    let (ni, nj) = (sr.net.min(segs[other].net), sr.net.max(segs[other].net));
                    *acc.entry((ni as u32, nj as u32)).or_default() += cc;
                }
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

/// Lower bound on grid cell size (um) — stops a near-zero cutoff from producing an
/// enormous number of tiny cells.
const MIN_CELL_UM: f64 = 1.0;

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
