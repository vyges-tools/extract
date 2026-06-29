//! Analytic RC model — geometry + rules → lumped per-net parasitics.
//!
//! v0 is a *rule-based* lumped extractor: per net, resistance is the sum of the
//! per-segment wire resistance plus (via count x per-via ohm); capacitance is the
//! sum of (segment length x layer fF/um) grounded cap. Wire resistance is
//! **width-dependent** when the deck supplies a per-layer sheet resistance
//! (`rsheet`): `R = rsheet · length / width`, with the width taken from the LEF
//! routing width — so a wider wire is correctly less resistive. Without `rsheet`
//! it falls back to the width-blind `res · length`. This is the same shape
//! OpenRCX's pattern path produces, at a coarser fidelity. The pi-model split and
//! coupling-capacitance terms are the correlated upgrade (see
//! `engine::ExtractError::FieldSolverNotFound`); the rules deck already carries a
//! `coupling` column so the model can grow into it without a format change.
//!
//! Pure std — fully unit-tested offline.

use crate::def::DefNet;
use crate::rules::RcRules;

#[derive(Debug, Clone)]
pub struct NetParasitics {
    pub name: String,
    pub pins: Vec<(String, String)>, // (instance, pin)
    pub res_ohm: f64,
    pub cap_ff: f64,
}

#[derive(Debug)]
pub struct RcError(pub String);

impl std::fmt::Display for RcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rc error: {}", self.0)
    }
}
impl std::error::Error for RcError {}

/// Extract one net's lumped R/C. Errors on a layer with no rule (so under-
/// extraction is loud, not silent). `widths` maps layer → routing width (um, from
/// the LEF) for the width-dependent `rsheet · len / width` resistance model; an
/// empty map keeps the width-blind `res · len` behaviour.
pub fn extract_net(
    net: &DefNet,
    rules: &RcRules,
    widths: &std::collections::BTreeMap<String, f64>,
) -> Result<NetParasitics, RcError> {
    let mut res_ohm = 0.0;
    let mut cap_ff = 0.0;
    for seg in &net.segments {
        let l = rules.layer(&seg.layer).ok_or_else(|| {
            RcError(format!("net {}: no rule for layer {:?}", net.name, seg.layer))
        })?;
        let w = widths.get(&seg.layer).copied().unwrap_or(0.0);
        res_ohm += rules.wire_res(&seg.layer, seg.len_um(), w).unwrap_or(0.0);
        cap_ff += seg.len_um() * l.cap_per_um; // grounded cap; coupling is separate (see coupling.rs)
    }
    res_ohm += net.vias as f64 * rules.via_res;
    Ok(NetParasitics {
        name: net.name.clone(),
        pins: net.pins.clone(),
        res_ohm,
        cap_ff,
    })
}
