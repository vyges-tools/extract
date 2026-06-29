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
//! Pure std — fully unit-tested offline.

use std::collections::{BTreeMap, HashMap};

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
pub fn extract_coupling(
    nets: &[DefNet],
    rules: &RcRules,
    widths: &BTreeMap<String, f64>,
    thicknesses: &BTreeMap<String, f64>,
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

    let mut acc: BTreeMap<(String, String), f64> = BTreeMap::new();
    for (id, sr) in segs.iter().enumerate() {
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
                *acc.entry((nets[ni].name.clone(), nets[nj].name.clone())).or_default() += cc;
            }
        }
    }
    acc.into_iter()
        .filter(|(_, c)| *c > 0.0)
        .map(|((a, b), cap_ff)| CouplingCap { a, b, cap_ff })
        .collect()
}

/// Lower bound on grid cell size (um) — stops a near-zero cutoff from producing an
/// enormous number of tiny cells.
const MIN_CELL_UM: f64 = 1.0;
