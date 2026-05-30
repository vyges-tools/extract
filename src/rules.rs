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
    pub coupling_per_um: f64, // fF / um (reserved for correlated extraction)
}

#[derive(Debug, Clone)]
pub struct RcRules {
    pub layers: BTreeMap<String, LayerRc>,
    pub via_res: f64, // default ohm per via cut
}

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
            let name = toks[0].to_string();
            if toks.len() < 3 {
                return Err(RulesError(format!(
                    "layer {name:?}: expected `name res cap [coupling]`"
                )));
            }
            let layer = LayerRc {
                res_per_um: num(toks[1], &format!("{name} res"))?,
                cap_per_um: num(toks[2], &format!("{name} cap"))?,
                coupling_per_um: match toks.get(3) {
                    Some(t) => num(t, &format!("{name} coupling"))?,
                    None => 0.0,
                },
            };
            layers.insert(name, layer);
        }
        if layers.is_empty() {
            return Err(RulesError("no layers defined".into()));
        }
        Ok(RcRules { layers, via_res })
    }

    pub fn load(path: &str) -> Result<RcRules, RulesError> {
        let text = std::fs::read_to_string(path).map_err(|e| RulesError(format!("{path}: {e}")))?;
        RcRules::parse(&text)
    }

    pub fn layer(&self, name: &str) -> Option<&LayerRc> {
        self.layers.get(name)
    }
}
