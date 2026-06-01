//! Per-layer RC rules deck — the technology coefficients the extractor applies.
//!
//! A `.rules` file is a small whitespace table (std-only parser — no deps).
//! Comments start with `#`. Two line shapes:
//!
//! ```text
//! # layer  res(ohm/um)  cap(fF/um)  [coupling(fF/um)]
//! met1     0.125        0.078       0.050
//! met2     0.125        0.072       0.044
//! via      5.0                                  # default per-via resistance (ohm)
//! ```
//!
//! `res` is sheet resistance reduced to ohm-per-micron at the layer's nominal
//! routing width; `cap` is grounded capacitance per micron; `coupling` (per
//! micron) is recorded for the correlated upgrade but not yet folded into the
//! v0 lumped total. A `via <ohm>` line sets the default per-via resistance.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default)]
pub struct LayerRc {
    pub res_per_um: f64,      // ohm / um
    pub cap_per_um: f64,      // fF / um (grounded)
    pub coupling_per_um: f64, // fF / um of parallel run, at the reference spacing s_ref
    pub s_ref: f64,           // um — spacing at which coupling_per_um is specified
}

#[derive(Debug, Clone)]
pub struct RcRules {
    pub layers: BTreeMap<String, LayerRc>,
    pub via_res: f64,        // default ohm per via cut
    pub couple_cutoff: f64,  // um — ignore lateral coupling beyond this gap
    /// Effective permittivity for the field-kernel coupling (`eps_r <v>`). When > 0
    /// and a metal thickness is known, coupling is `eps_r·eps0·T/gap` (the geometry-
    /// derived M5 model) instead of the per-layer `coupling` coefficient.
    pub eps_r: f64,
    /// Areal coupling (fF/um^2) between a pair of (different) layers whose
    /// footprints overlap — keyed by the layer names sorted ascending.
    pub interlayer: BTreeMap<(String, String), f64>,
}

/// Order-independent key for a layer pair.
fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// Default reference spacing (um) when a layer omits its 5th column.
const DEFAULT_S_REF: f64 = 0.2;
/// Default lateral-coupling cutoff distance (um).
const DEFAULT_COUPLE_CUTOFF: f64 = 2.0;

#[derive(Debug)]
pub struct RulesError(pub String);

impl std::fmt::Display for RulesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rules error: {}", self.0)
    }
}
impl std::error::Error for RulesError {}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn num(tok: &str, what: &str) -> Result<f64, RulesError> {
    tok.parse::<f64>().map_err(|_| RulesError(format!("{what}: not a number: {tok:?}")))
}

impl RcRules {
    pub fn parse(text: &str) -> Result<RcRules, RulesError> {
        let mut layers = BTreeMap::new();
        let mut via_res = 0.0;
        let mut couple_cutoff = DEFAULT_COUPLE_CUTOFF;
        let mut eps_r = 0.0;
        let mut interlayer = BTreeMap::new();
        for raw in text.lines() {
            let toks: Vec<&str> = strip_comment(raw).split_whitespace().collect();
            if toks.is_empty() {
                continue;
            }
            // optional leading `layer` keyword
            let toks: Vec<&str> = if toks[0].eq_ignore_ascii_case("layer") {
                toks[1..].to_vec()
            } else {
                toks
            };
            if toks.is_empty() {
                continue;
            }
            if toks[0].eq_ignore_ascii_case("via") {
                via_res = num(toks.get(1).copied().unwrap_or(""), "via res")?;
                continue;
            }
            if toks[0].eq_ignore_ascii_case("couple_cutoff") {
                couple_cutoff = num(toks.get(1).copied().unwrap_or(""), "couple_cutoff")?;
                continue;
            }
            if toks[0].eq_ignore_ascii_case("eps_r") {
                eps_r = num(toks.get(1).copied().unwrap_or(""), "eps_r")?;
                continue;
            }
            if toks[0].eq_ignore_ascii_case("interlayer") {
                let a = toks.get(1).copied().unwrap_or("");
                let b = toks.get(2).copied().unwrap_or("");
                if a.is_empty() || b.is_empty() {
                    return Err(RulesError("interlayer needs `layerA layerB fF/um2`".into()));
                }
                let c = num(toks.get(3).copied().unwrap_or(""), "interlayer coeff")?;
                interlayer.insert(pair_key(a, b), c);
                continue;
            }
            let name = toks[0].to_string();
            if toks.len() < 3 {
                return Err(RulesError(format!(
                    "layer {name:?}: expected `name res cap [coupling [s_ref]]`"
                )));
            }
            let layer = LayerRc {
                res_per_um: num(toks[1], &format!("{name} res"))?,
                cap_per_um: num(toks[2], &format!("{name} cap"))?,
                coupling_per_um: match toks.get(3) {
                    Some(t) => num(t, &format!("{name} coupling"))?,
                    None => 0.0,
                },
                s_ref: match toks.get(4) {
                    Some(t) => num(t, &format!("{name} s_ref"))?,
                    None => DEFAULT_S_REF,
                },
            };
            layers.insert(name, layer);
        }
        if layers.is_empty() {
            return Err(RulesError("no layers defined".into()));
        }
        Ok(RcRules { layers, via_res, couple_cutoff, eps_r, interlayer })
    }

    pub fn load(path: &str) -> Result<RcRules, RulesError> {
        let text = std::fs::read_to_string(path).map_err(|e| RulesError(format!("{path}: {e}")))?;
        RcRules::parse(&text)
    }

    pub fn layer(&self, name: &str) -> Option<&LayerRc> {
        self.layers.get(name)
    }

    /// Areal coupling (fF/um^2) between two layers, if defined (order-independent).
    pub fn interlayer(&self, a: &str, b: &str) -> Option<f64> {
        self.interlayer.get(&pair_key(a, b)).copied()
    }
}
