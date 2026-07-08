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
//! rsheet   met1 0.125                           # sheet resistance (ohm/square)
//! ```
//!
//! `res` is resistance per micron at the layer's nominal routing width; `cap` is
//! grounded capacitance per micron; `coupling` (per micron) is recorded for the
//! correlated upgrade. A `via <ohm>` line sets the default per-via resistance. An
//! optional `rsheet <layer> <ohm/square>` line switches that layer to the
//! **width-dependent** resistance `rsheet · length / width` (width from the LEF),
//! so a wider wire is correctly less resistive; without it the width-blind `res`
//! column is used.

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
    /// Conditional ground-cap shielding fraction (`shield_k <0..1>`). A net's
    /// coupling `Cc` is field that would otherwise terminate on ground as fringe, so
    /// the grounded cap is reduced by `shield_k · Cc_net` (charge conservation),
    /// making it neighbour-dependent. 0 disables (back-compat).
    pub shield_k: f64,
    /// Per-layer metal height above the ground plane (um), for the fringe-corrected
    /// field kernel (`height <layer> <um>`). Empty -> bare parallel-plate coupling.
    pub heights: BTreeMap<String, f64>,
    /// Areal coupling (fF/um^2) between a pair of (different) layers whose
    /// footprints overlap — keyed by the layer names sorted ascending.
    pub interlayer: BTreeMap<(String, String), f64>,
    /// Per-layer **sheet resistance** (ohm/square, `rsheet <layer> <ohm_sq>`). When
    /// present, wire resistance is the width-dependent `rsheet · length / width`
    /// (width from the LEF routing width, or a per-segment width if one is known)
    /// instead of the width-blind `res · length`. Empty -> the `res` column is used.
    pub rsheet: BTreeMap<String, f64>,
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
    /// Derive per-layer RC from a PDK tech LEF — real `RESISTANCE`/`CAPACITANCE`
    /// values for **every** `TYPE ROUTING` layer, so the metal stack is discovered
    /// dynamically (5, 6, 9 … metals) rather than hand-listed. LEF capacitances are
    /// picofarads; converted to fF/µm. A CUT layer's per-cut `RESISTANCE` feeds the
    /// lumped via resistance (median across cuts, robust to the diffusion contact).
    pub fn from_lef(lef: &vyges_loom::lef::Lef) -> RcRules {
        let mut layers = BTreeMap::new();
        let mut rsheet = BTreeMap::new();
        let mut cut_res: Vec<f64> = Vec::new();
        for (name, l) in &lef.layers {
            if l.routing {
                let w = if l.width_um > 0.0 { l.width_um } else { 1.0 };
                if l.rpersq > 0.0 {
                    rsheet.insert(name.clone(), l.rpersq);
                }
                // cap (fF/µm) = area-cap·width + 2·fringe; LEF caps are pF -> ·1000.
                let cap_per_um = (l.cpersqdist * w + 2.0 * l.edge_cap) * 1000.0;
                layers.insert(
                    name.clone(),
                    LayerRc { res_per_um: l.rpersq, cap_per_um, coupling_per_um: 0.0, s_ref: 0.0 },
                );
            } else if l.cut_res > 0.0 {
                cut_res.push(l.cut_res);
            }
        }
        cut_res.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let via_res = if cut_res.is_empty() { 0.0 } else { cut_res[cut_res.len() / 2] };
        RcRules {
            layers,
            via_res,
            couple_cutoff: 2.0,
            eps_r: 0.0,
            shield_k: 0.0,
            heights: BTreeMap::new(),
            interlayer: BTreeMap::new(),
            rsheet,
        }
    }

    /// Derive lumped per-layer RC from an **OpenRCX captable** (`*.rcx_rules` /
    /// `rcx_patterns.rules`) — the RC source for PDKs whose tech LEF is geometry-only
    /// (e.g. asap7). This is a MINIMAL read of the pattern model, not a full parser:
    /// per metal we take the representative `RESOVER` value as resistance and the
    /// `OVER` (ground) plus the nearest `UNDER` (immediate upper layer) as the grounded
    /// capacitance. The captable indexes layers by stack position (`Metal N`), mapped
    /// to a name via `routing_order` (the LEF stack).
    pub fn from_captable(routing_order: &[String], rcx: &str) -> RcRules {
        let (mut res, mut over, mut under): (BTreeMap<usize, f64>, BTreeMap<usize, f64>, BTreeMap<usize, f64>) =
            Default::default();
        let mut block: Option<(usize, u8)> = None; // (metal index, 0=res 1=over 2=under)
        let mut taken = false;
        for line in rcx.lines() {
            let t: Vec<&str> = line.split_whitespace().collect();
            // block header: `Metal <k> RESOVER|OVER|UNDER <ctx>`
            if t.len() >= 4 && t[0] == "Metal" {
                if let Ok(k) = t[1].parse::<usize>() {
                    let kind = match t[2] {
                        "RESOVER" => Some(0u8),
                        "OVER" => Some(1u8),
                        "UNDER" => Some(2u8),
                        _ => None,
                    };
                    block = match kind {
                        // RESOVER/OVER: only the base `0` context; UNDER: the first
                        // (nearest-upper) block for this metal.
                        Some(kd) if (kd != 2 && t[3] == "0") || (kd == 2 && !under.contains_key(&k)) => Some((k, kd)),
                        _ => None,
                    };
                    taken = false;
                    continue;
                }
            }
            // first 4-column data row of the active block: the value is the last column.
            if let Some((k, kd)) = block {
                if !taken && t.len() == 4 {
                    if let Ok(v) = t[3].parse::<f64>() {
                        match kd {
                            0 => drop(res.entry(k).or_insert(v)),
                            1 => drop(over.entry(k).or_insert(v)),
                            _ => drop(under.entry(k).or_insert(v)),
                        }
                        taken = true;
                    }
                }
            }
        }
        let mut layers = BTreeMap::new();
        let mut rsheet = BTreeMap::new();
        for (&k, &r) in &res {
            if let Some(name) = routing_order.get(k.wrapping_sub(1)) {
                let c = over.get(&k).copied().unwrap_or(0.0) + under.get(&k).copied().unwrap_or(0.0);
                layers.insert(name.clone(), LayerRc { res_per_um: r, cap_per_um: c, coupling_per_um: 0.0, s_ref: 0.0 });
                rsheet.insert(name.clone(), r);
            }
        }
        RcRules {
            layers,
            via_res: 0.0,
            couple_cutoff: 2.0,
            eps_r: 0.0,
            shield_k: 0.0,
            heights: BTreeMap::new(),
            interlayer: BTreeMap::new(),
            rsheet,
        }
    }

    /// Serialise to the RC deck text (so a derived ruleset can be cached as a file).
    pub fn to_deck(&self) -> String {
        let mut s = String::from(
            "# vyges-extract RC rules — DERIVED from the PDK tech LEF; do not hand-edit.\n\
             # Regenerate with `vyges-extract run <job> --pdk <name> --refresh`.\n\
             # layer  res(ohm/um)  cap(fF/um)  coupling(fF/um)  s_ref(um)\n",
        );
        for (name, l) in &self.layers {
            s.push_str(&format!("{} {} {} {} {}\n", name, l.res_per_um, l.cap_per_um, l.coupling_per_um, l.s_ref));
        }
        s.push_str(&format!("via {}\n", self.via_res));
        s.push_str(&format!("couple_cutoff {}\n", self.couple_cutoff));
        for (name, r) in &self.rsheet {
            s.push_str(&format!("rsheet {} {}\n", name, r));
        }
        s
    }

    pub fn parse(text: &str) -> Result<RcRules, RulesError> {
        let mut layers = BTreeMap::new();
        let mut via_res = 0.0;
        let mut couple_cutoff = DEFAULT_COUPLE_CUTOFF;
        let mut eps_r = 0.0;
        let mut shield_k = 0.0;
        let mut heights = BTreeMap::new();
        let mut interlayer = BTreeMap::new();
        let mut rsheet = BTreeMap::new();
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
            if toks[0].eq_ignore_ascii_case("shield_k") {
                shield_k = num(toks.get(1).copied().unwrap_or(""), "shield_k")?;
                continue;
            }
            if toks[0].eq_ignore_ascii_case("rsheet") {
                let layer = toks.get(1).copied().unwrap_or("");
                if layer.is_empty() {
                    return Err(RulesError("rsheet needs `layer <ohm/square>`".into()));
                }
                rsheet.insert(layer.to_string(), num(toks.get(2).copied().unwrap_or(""), "rsheet")?);
                continue;
            }
            if toks[0].eq_ignore_ascii_case("height") {
                let layer = toks.get(1).copied().unwrap_or("");
                if layer.is_empty() {
                    return Err(RulesError("height needs `layer <um>`".into()));
                }
                heights.insert(layer.to_string(), num(toks.get(2).copied().unwrap_or(""), "height")?);
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
        Ok(RcRules { layers, via_res, couple_cutoff, eps_r, shield_k, heights, interlayer, rsheet })
    }

    pub fn load(path: &str) -> Result<RcRules, RulesError> {
        let text = std::fs::read_to_string(path).map_err(|e| RulesError(format!("{path}: {e}")))?;
        RcRules::parse(&text)
    }

    pub fn layer(&self, name: &str) -> Option<&LayerRc> {
        self.layers.get(name)
    }

    /// Resistance (ohm) of a `len_um`-long wire on `layer` that is `width_um` wide.
    /// When the layer has a **sheet resistance** and a positive width, this is the
    /// width-dependent `rsheet · len / width`; otherwise it falls back to the
    /// width-blind `res · len`. `None` only if the layer has no rule at all (so
    /// under-extraction stays a hard error, never silent).
    pub fn wire_res(&self, layer: &str, len_um: f64, width_um: f64) -> Option<f64> {
        let l = self.layers.get(layer)?;
        match self.rsheet.get(layer) {
            Some(&rs) if width_um > 0.0 => Some(rs * len_um / width_um),
            _ => Some(len_um * l.res_per_um),
        }
    }

    /// Areal coupling (fF/um^2) between two layers, if defined (order-independent).
    pub fn interlayer(&self, a: &str, b: &str) -> Option<f64> {
        self.interlayer.get(&pair_key(a, b)).copied()
    }
}
