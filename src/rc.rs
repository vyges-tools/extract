//! Analytic RC model — geometry + rules → lumped per-net parasitics.
//!
//! v0 is a *rule-based* lumped extractor: per net, resistance is the sum of
//! (segment length x layer ohm/um) plus (via count x per-via ohm); capacitance
//! is the sum of (segment length x layer fF/um) grounded cap. This is the same
//! shape OpenRCX's pattern path produces, at a coarser fidelity. The pi-model
//! split and coupling-capacitance terms are the correlated upgrade (see
//! `engine::ExtractError::FieldSolverNotFound`); the rules deck already carries
//! a `coupling` column so the model can grow into it without a format change.
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
/// extraction is loud, not silent).
pub fn extract_net(net: &DefNet, rules: &RcRules) -> Result<NetParasitics, RcError> {
    let mut res_ohm = 0.0;
    let mut cap_ff = 0.0;
    for seg in &net.segments {
        let l = rules.layer(&seg.layer).ok_or_else(|| {
            RcError(format!("net {}: no rule for layer {:?}", net.name, seg.layer))
        })?;
        res_ohm += seg.len_um() * l.res_per_um;
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
