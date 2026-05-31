//! Extraction engine: job -> DEF + rules -> per-net parasitics -> SPEF.
//!
//! v0 runs the pure-std rule-based path end to end, no subprocess (so it works
//! offline). The `FieldSolverNotFound` variant is reserved for the *correlated*
//! upgrade: when extraction is asked to field-solve coupling capacitance and
//! fit against golden patterns, that step shells out to the EDA environment,
//! mirroring how `vyges-char` degrades when `ngspice` is absent.

use crate::coupling::{self, CouplingCap};
use crate::def::{self, Def};
use crate::job::ExtractJob;
use crate::rc::{self, NetParasitics};
use crate::rules::RcRules;
use crate::spef::{self, Units};

/// Full extraction result: per-net parasitics + inter-net coupling caps.
#[derive(Debug, Clone)]
pub struct Extraction {
    pub nets: Vec<NetParasitics>,
    pub couplings: Vec<CouplingCap>,
}

#[derive(Debug)]
pub enum ExtractError {
    Parse(String),
    Io(String),
    /// Reserved: the correlated (field-solve) path needs the EDA environment.
    FieldSolverNotFound,
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Parse(m) => write!(f, "parse error: {m}"),
            ExtractError::Io(m) => write!(f, "io error: {m}"),
            ExtractError::FieldSolverNotFound => write!(
                f,
                "field solver not found on PATH. Correlated extraction needs \
                 the EDA environment (VyBox / build host); the rule-based path \
                 runs without it."
            ),
        }
    }
}
impl std::error::Error for ExtractError {}

/// Load the job's DEF + rules; extract per-net parasitics and coupling caps.
pub fn extract(job: &ExtractJob) -> Result<Extraction, ExtractError> {
    let d: Def = def::load(&job.resolve(&job.def)).map_err(|e| ExtractError::Parse(e.to_string()))?;
    let r: RcRules =
        RcRules::load(&job.resolve(&job.rules)).map_err(|e| ExtractError::Parse(e.to_string()))?;
    let nets = d
        .nets
        .iter()
        .map(|n| rc::extract_net(n, &r).map_err(|e| ExtractError::Parse(e.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    let couplings = coupling::extract_coupling(&d.nets, &r);
    Ok(Extraction { nets, couplings })
}

/// Full run: extract and render a `.spef`.
pub fn run_to_spef(job: &ExtractJob) -> Result<String, ExtractError> {
    let ex = extract(job)?;
    Ok(spef::render(&job.design, &Units::default(), None, &ex.nets, &ex.couplings))
}
