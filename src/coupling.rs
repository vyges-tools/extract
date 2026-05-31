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
//! Pure std — fully unit-tested offline.

use std::collections::BTreeMap;

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

/// Coupling caps between every pair of nets — one aggregated entry per net pair.
///
/// `widths` maps layer → default routing width (um, from LEF). When present the
/// gap is edge-to-edge; when a layer is absent (width 0) it degrades to the
/// centerline distance — so an empty map reproduces the pre-LEF behaviour.
pub fn extract_coupling(
    nets: &[DefNet],
    rules: &RcRules,
    widths: &BTreeMap<String, f64>,
) -> Vec<CouplingCap> {
    let width = |layer: &str| widths.get(layer).copied().unwrap_or(0.0);
    let mut acc: BTreeMap<(String, String), f64> = BTreeMap::new();
    for i in 0..nets.len() {
        for j in (i + 1)..nets.len() {
            let (na, nb) = (&nets[i], &nets[j]);
            let mut cc = 0.0;
            for sa in &na.segments {
                for sb in &nb.segments {
                    if sa.layer == sb.layer {
                        // lateral coupling: parallel same-layer runs, edge-to-edge gap
                        let Some((ov, center_gap)) = overlap_gap(sa, sb) else { continue };
                        let Some(l) = rules.layer(&sa.layer) else { continue };
                        if l.coupling_per_um <= 0.0 {
                            continue;
                        }
                        let gap = center_gap - width(&sa.layer);
                        if gap > rules.couple_cutoff {
                            continue;
                        }
                        cc += l.coupling_per_um * ov * (l.s_ref / gap.max(l.s_ref));
                    } else if let Some(coeff) = rules.interlayer(&sa.layer, &sb.layer) {
                        // inter-layer (crossover) coupling: areal cap over the
                        // footprint overlap (needs widths -> zero without a LEF).
                        let area = rect_overlap(
                            sa.footprint(width(&sa.layer)),
                            sb.footprint(width(&sb.layer)),
                        );
                        cc += coeff * area;
                    }
                }
            }
            if cc > 0.0 {
                acc.insert((na.name.clone(), nb.name.clone()), cc);
            }
        }
    }
    acc.into_iter().map(|((a, b), cap_ff)| CouplingCap { a, b, cap_ff }).collect()
}
