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
        let Some((ov, center_gap)) = overlap_gap(sa, sb) else { return 0.0 };
        let Some(l) = rules.layer(&sa.layer) else { return 0.0 };
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
            // rule-based coefficient (falls back when no eps_r / thickness)
            l.coupling_per_um * ov * (l.s_ref / gap.max(l.s_ref))
        } else {
            0.0
        }
    } else if let Some(coeff) = rules.interlayer(&sa.layer, &sb.layer) {
        // inter-layer (crossover) coupling: areal cap over the footprint overlap
        // (needs widths -> zero without a LEF).
        coeff * rect_overlap(sa.footprint(width(&sa.layer)), sb.footprint(width(&sb.layer)))
    } else {
        0.0
    }
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
    let partials: Vec<HashMap<(u32, u32), f64>> =
        bands.par_iter().map(|&(lo, hi)| scan_band(lo, hi)).collect();

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
