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
//! at the reference spacing `s_ref`, falling off ~1/gap. Centerline coordinates
//! give the gap here; true edge-to-edge gap (needs LEF wire widths) and a
//! field-solved 2.5-D kernel are the M5 upgrades that replace this model behind
//! the same SPEF output.
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

/// Coupling caps between every pair of nets — one aggregated entry per net pair.
pub fn extract_coupling(nets: &[DefNet], rules: &RcRules) -> Vec<CouplingCap> {
    let mut acc: BTreeMap<(String, String), f64> = BTreeMap::new();
    for i in 0..nets.len() {
        for j in (i + 1)..nets.len() {
            let (na, nb) = (&nets[i], &nets[j]);
            let mut cc = 0.0;
            for sa in &na.segments {
                for sb in &nb.segments {
                    let Some((ov, gap)) = overlap_gap(sa, sb) else { continue };
                    if gap <= 0.0 || gap > rules.couple_cutoff {
                        continue;
                    }
                    let Some(l) = rules.layer(&sa.layer) else { continue };
                    if l.coupling_per_um <= 0.0 {
                        continue;
                    }
                    cc += l.coupling_per_um * ov * (l.s_ref / gap.max(l.s_ref));
                }
            }
            if cc > 0.0 {
                acc.insert((na.name.clone(), nb.name.clone()), cc);
            }
        }
    }
    acc.into_iter().map(|((a, b), cap_ff)| CouplingCap { a, b, cap_ff }).collect()
}
