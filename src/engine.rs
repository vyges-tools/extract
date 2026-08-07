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

/// The power grid as occluding metal, in microns.
///
/// `SPECIALNETS` geometry is parsed in DB units with an explicit strap width; the
/// coupling pass wants microns and a per-segment width, since a PDN strap is nothing
/// like a default-width signal wire. Returns empty when the DEF carries no PDN (or no
/// `UNITS DISTANCE MICRONS`), which leaves the coupling pass exactly as it was.
pub(crate) fn power_blockers(d: &Def) -> Vec<def::Segment> {
    let dbu = if d.units_per_um > 0.0 {
        d.units_per_um
    } else {
        d.dbu
    };
    if dbu <= 0.0 {
        return Vec::new();
    }
    d.power_nets
        .iter()
        .flat_map(|n| n.segs.iter())
        .map(|s| def::Segment {
            layer: s.layer.clone(),
            x0: s.x1 as f64 / dbu,
            y0: s.y1 as f64 / dbu,
            x1: s.x2 as f64 / dbu,
            y1: s.y2 as f64 / dbu,
            width_um: s.width_dbu / dbu,
        })
        .collect()
}

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
    emit_input_coverage(&d, &r, &lef, job.lef.is_some());
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
    let outcomes: Vec<tree::Outcome> = d
        .nets
        .par_iter()
        .map(|n| tree::build_network(n, &r, &lef.widths, &lef.vias))
        .collect();
    // A net whose geometry is present but will not resolve into ONE network falls back to
    // the star, which is connected by construction. That is the right output, but it is a
    // defect upstream of here and must not vanish into the same silence as "this net has no
    // routing" — so it is counted and said out loud.
    let disconnected: Vec<&str> = d
        .nets
        .iter()
        .zip(&outcomes)
        .filter(|(_, o)| matches!(o, tree::Outcome::Disconnected { .. }))
        .map(|(n, _)| n.name.as_str())
        .collect();
    if !disconnected.is_empty() {
        use vyges_events::{Event, Severity};
        vyges_events::emit(
            &Event::new(
                "vyges-extract",
                Severity::Warn,
                format!(
                    "{} of {} net(s) had routing that would not resolve into a single RC \
                     network — emitted as a lumped star instead (e.g. {})",
                    disconnected.len(),
                    d.nets.len(),
                    disconnected
                        .iter()
                        .take(3)
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with_code("EXTRACT-RC-DISCONNECTED")
            .with_objects(
                disconnected
                    .iter()
                    .take(20)
                    .map(|n| format!("net:{n}"))
                    .collect::<Vec<_>>(),
            ),
        );
    }
    let trees: Vec<Option<RcNetwork>> = outcomes.into_iter().map(|o| o.built()).collect();
    lap!("per-net trees");
    // The power grid never appears in the coupling output — an AC ground takes field as
    // grounded capacitance — but it is metal, and metal blocks line of sight. On sky130
    // standard-cell rows a met1 rail runs through every row, so two signals on either
    // side of one must not couple through it.
    let blockers = power_blockers(&d);
    let couplings =
        coupling::extract_coupling_blocked(&d.nets, &r, &lef.widths, &lef.thicknesses, &blockers);
    lap!("coupling");
    // The macro resets the clock after printing, so the final lap's reset is dead by
    // construction. Read it once rather than restructure a debugging aid around a lint.
    let _ = t;
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

/// Report how much of the layout the extraction inputs actually describe.
///
/// Extraction's silent failure is not a file that fails to parse — it is a net with no routing
/// geometry, which extracts to nothing and reports a parasitic of essentially zero, and a layer
/// the rules do not define, whose segments are then extracted against a default nobody chose.
/// Both produce a SPEF that looks complete.
///
/// Counting comes from the same shared module the other engines use where it applies; these two
/// are extraction-specific and live here.
fn emit_input_coverage(d: &Def, rules: &RcRules, lef: &Lef, lef_given: bool) {
    use std::collections::BTreeSet;
    use vyges_events::{Event, Severity};
    let emit = |attention: bool, code: &str, msg: String| {
        let sev = if attention {
            Severity::Warn
        } else {
            Severity::Info
        };
        vyges_events::emit(&Event::new("vyges-extract", sev, msg).with_code(code));
    };

    let routed = d.nets.iter().filter(|n| !n.segments.is_empty()).count();
    let bare = d.nets.len() - routed;
    emit(
        d.nets.is_empty() || bare > 0,
        "EXTRACT-DEF",
        format!(
            "DEF: {} signal net(s), {routed} with routing geometry, {bare} with none \
             (those extract to nothing), {} placed instance(s)",
            d.nets.len(),
            d.comps.len()
        ),
    );

    // Every layer the routing actually uses, against the layers the rules and LEF describe.
    // A layer present in the geometry and absent from the rules is extracted against a default,
    // which is the quiet way to get a whole metal's resistance wrong.
    let used: BTreeSet<&str> = d
        .nets
        .iter()
        .flat_map(|n| n.segments.iter())
        .map(|sg| sg.layer.as_str())
        .collect();
    let no_rule: Vec<&str> = used
        .iter()
        .copied()
        .filter(|l| !rules.layers.contains_key(*l))
        .collect();
    let no_width: Vec<&str> = used
        .iter()
        .copied()
        .filter(|l| !lef.widths.contains_key(*l))
        .collect();

    let mut notes = Vec::new();
    if !no_rule.is_empty() {
        notes.push(format!("no RC rule for {:?}", no_rule));
    }
    if !lef_given {
        // Not a defect — the LEF is optional — but it changes what the numbers mean, and that
        // is worth one line rather than a footnote in a manual.
        notes.push("no LEF given, so widths are defaults rather than the technology's".into());
    } else if !no_width.is_empty() {
        notes.push(format!("no LEF width for {:?}", no_width));
    }
    let base = format!(
        "layers: {} used by the routing, {} with RC rules, {} with LEF widths",
        used.len(),
        used.len() - no_rule.len(),
        used.len() - no_width.len()
    );
    emit(
        !no_rule.is_empty(),
        "EXTRACT-LAYERS",
        if notes.is_empty() {
            base
        } else {
            format!("{base} — {}", notes.join("; "))
        },
    );
}

#[cfg(test)]
mod power_blocker_tests {
    use super::*;
    use crate::def::{NetGeom, Seg};

    fn strap(layer: &str, w: f64, x1: i64, y1: i64, x2: i64, y2: i64) -> Seg {
        Seg { layer: layer.into(), width_dbu: w, x1, y1, x2, y2 }
    }

    /// `SPECIALNETS` geometry arrives in DB units with an explicit strap width; the coupling
    /// pass wants microns. Getting the scale wrong puts the whole power grid in the wrong
    /// place, where it blocks nothing and nothing fails — so pin the conversion.
    #[test]
    fn power_geometry_converts_from_dbu_to_microns() {
        let d = Def {
            units_per_um: 1000.0,
            dbu: 1000.0,
            power_nets: vec![NetGeom {
                name: "VGND".into(),
                use_power: false,
                // a met1 FOLLOWPIN rail exactly as OpenLane writes it: 480 DBU wide
                segs: vec![strap("met1", 480.0, 5520, 48960, 54280, 48960)],
                ..Default::default()
            }],
            ..Default::default()
        };
        let b = power_blockers(&d);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].layer, "met1");
        assert!((b[0].x0 - 5.520).abs() < 1e-9, "x0 = {}", b[0].x0);
        assert!((b[0].y0 - 48.960).abs() < 1e-9, "y0 = {}", b[0].y0);
        assert!((b[0].x1 - 54.280).abs() < 1e-9, "x1 = {}", b[0].x1);
        // The strap's own width, NOT the layer default — a rail is 3.4x a min-width met1 wire,
        // and the difference is most of what it blocks.
        assert!((b[0].width_um - 0.480).abs() < 1e-9, "width = {}", b[0].width_um);
        assert!(b[0].is_horizontal(), "a FOLLOWPIN rail runs along the row");
    }

    /// Every strap of every power net takes part, on every layer — not just the first net.
    #[test]
    fn all_power_nets_and_layers_contribute() {
        let d = Def {
            units_per_um: 1000.0,
            dbu: 1000.0,
            power_nets: vec![
                NetGeom {
                    name: "VGND".into(),
                    segs: vec![
                        strap("met1", 480.0, 0, 1000, 10000, 1000),
                        strap("met4", 1600.0, 5000, 0, 5000, 10000),
                    ],
                    ..Default::default()
                },
                NetGeom {
                    name: "VPWR".into(),
                    use_power: true,
                    segs: vec![strap("met1", 480.0, 0, 3720, 10000, 3720)],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let b = power_blockers(&d);
        assert_eq!(b.len(), 3);
        assert!(b.iter().any(|s| s.layer == "met4" && (s.width_um - 1.6).abs() < 1e-9));
        assert_eq!(b.iter().filter(|s| s.layer == "met1").count(), 2);
    }

    /// A DEF with no `UNITS DISTANCE MICRONS` cannot be converted, and guessing a scale would
    /// silently misplace the grid. No units -> no blockers, i.e. exactly the old behaviour.
    #[test]
    fn no_units_means_no_blockers_rather_than_a_guess() {
        let d = Def {
            units_per_um: 0.0,
            dbu: 0.0,
            power_nets: vec![NetGeom {
                name: "VGND".into(),
                segs: vec![strap("met1", 480.0, 0, 0, 10000, 0)],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(power_blockers(&d).is_empty());
    }
}
