//! Extraction engine: job -> DEF + rules -> per-net parasitics -> SPEF.
//!
//! v0 runs the pure-std rule-based path end to end, no subprocess (so it works
//! offline). The `FieldSolverNotFound` variant is reserved for the *correlated*
//! upgrade: when extraction is asked to field-solve coupling capacitance and
//! fit against golden patterns, that step shells out to the EDA environment,
//! mirroring how `vyges-char` degrades when `ngspice` is absent.

use rayon::prelude::*;

use crate::coupling::{self, CouplingCap};
use crate::def::{self, Def};
use crate::job::ExtractJob;
use crate::lef::Lef;
use crate::rc::{self, NetParasitics};
use crate::rules::RcRules;
use crate::spef::{self, Units};
use crate::tree::{self, RcNetwork};

/// Full extraction result: per-net parasitics + inter-net coupling caps.
///
/// `trees[i]` is the distributed RC network for `nets[i]` when the routing
/// geometry supports one (`None` -> the SPEF emitter uses the lumped star).
#[derive(Debug, Clone)]
pub struct Extraction {
    pub nets: Vec<NetParasitics>,
    pub trees: Vec<Option<RcNetwork>>,
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
    // Optional per-phase wall-clock breakdown (set VYGES_TIMING=1) — isolates the parallel
    // phases (per-net RC, coupling) from the serial DEF parse when benchmarking thread scaling.
    let timing = std::env::var_os("VYGES_TIMING").is_some();
    let mut t = std::time::Instant::now();
    macro_rules! lap {
        ($label:expr) => {
            if timing {
                eprintln!("[timing] {:<14} {:.2}s", $label, t.elapsed().as_secs_f64());
                t = std::time::Instant::now();
            }
        };
    }
    let d: Def =
        def::load(&job.resolve(&job.def)).map_err(|e| ExtractError::Parse(e.to_string()))?;
    lap!("parse DEF");
    let r: RcRules =
        RcRules::load(&job.resolve(&job.rules)).map_err(|e| ExtractError::Parse(e.to_string()))?;
    // LEF (optional) -> routing widths (width-dependent R + edge-to-edge coupling
    // gaps) + thicknesses (field kernel). Loaded before RC so resistance is width-aware.
    let lef = match &job.lef {
        Some(p) => Lef::load(&job.resolve(p)).map_err(|e| ExtractError::Parse(e.to_string()))?,
        None => Lef::default(),
    };
    // Per-net RC is independent across nets — extract them in parallel (rayon). The pool
    // size is set once in main() from `--threads`/`-j` (default: all cores). collect() into
    // a Result short-circuits on the first net that fails to parse, same as the serial path.
    let mut nets = d
        .nets
        .par_iter()
        .map(|n| {
            rc::extract_net(n, &r, &lef.widths).map_err(|e| ExtractError::Parse(e.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    lap!("per-net RC");
    // Distributed RC network per net (from routing geometry); None -> lumped star.
    let trees: Vec<Option<RcNetwork>> = d
        .nets
        .par_iter()
        .map(|n| tree::build_network(n, &r, &lef.widths))
        .collect();
    lap!("per-net trees");
    let couplings = coupling::extract_coupling(&d.nets, &r, &lef.widths, &lef.thicknesses);
    lap!("coupling");
    // Conditional ground-cap shielding: a net's coupling is field that would otherwise
    // be grounded fringe, so reduce its grounded cap by `shield_k · Cc_net` (charge
    // conservation) — making the ground cap neighbour-dependent, the spread lever.
    if r.shield_k > 0.0 {
        let mut cc_net: std::collections::BTreeMap<&str, f64> = std::collections::BTreeMap::new();
        for c in &couplings {
            *cc_net.entry(c.a.as_str()).or_default() += c.cap_ff;
            *cc_net.entry(c.b.as_str()).or_default() += c.cap_ff;
        }
        for net in &mut nets {
            if let Some(&cc) = cc_net.get(net.name.as_str()) {
                net.cap_ff = (net.cap_ff - r.shield_k * cc).max(0.0);
            }
        }
    }
    Ok(Extraction {
        nets,
        trees,
        couplings,
    })
}

/// Full run: extract and render a `.spef`.
pub fn run_to_spef(job: &ExtractJob) -> Result<String, ExtractError> {
    let ex = extract(job)?;
    Ok(spef::render_distributed(
        &job.design,
        &Units::default(),
        None,
        &ex.nets,
        &ex.trees,
        &ex.couplings,
        None,
    ))
}
