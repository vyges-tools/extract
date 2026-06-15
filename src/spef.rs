//! SPEF (IEEE 1481) emitter.
//!
//! Emits a Standard Parasitic Exchange Format file with a name map and, per
//! net, a `*D_NET` record: `*CONN` (the connected instance pins), `*CAP` (the
//! grounded + coupling capacitance), and `*RES` (the series resistance). The
//! net is emitted as a **per-pin RC tree** — a star rooted at the net node with
//! a trunk R/2 + near-half C to the driver and a branch R/2K + cap share to each
//! sink — which reduces to a pi for a single sink and a lump with no R. A
//! moment-weighted, geometry-aware tree is the refinement.
//!
//! Pure std — fully unit-tested offline. No timestamp is embedded unless one is
//! passed in, so an unchanged run is bit-identical (the M2 reproducibility
//! contract).

use std::collections::BTreeMap;

use crate::coupling::CouplingCap;
use crate::rc::NetParasitics;

#[derive(Debug, Clone)]
pub struct Units {
    pub time: String, // e.g. "1 PS"
    pub cap: String,  // e.g. "1 FF"
    pub res: String,  // e.g. "1 OHM"
}

impl Default for Units {
    fn default() -> Self {
        Units { time: "1 PS".into(), cap: "1 FF".into(), res: "1 OHM".into() }
    }
}

/// Assigns stable integer ids to names in first-seen order (deterministic).
struct NameMap {
    ids: BTreeMap<String, usize>,
    order: Vec<String>,
}

impl NameMap {
    fn new() -> NameMap {
        NameMap { ids: BTreeMap::new(), order: Vec::new() }
    }
    fn intern(&mut self, name: &str) -> usize {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = self.order.len() + 1;
        self.ids.insert(name.to_string(), id);
        self.order.push(name.to_string());
        id
    }
    fn id(&self, name: &str) -> Option<usize> {
        self.ids.get(name).copied()
    }
}

fn val(v: f64) -> String {
    format!("{v:.6}")
}

/// Machine-readable per-net parasitics + coupling summary (std-only, no deps).
pub fn render_json(design: &str, nets: &[NetParasitics], couplings: &[CouplingCap]) -> String {
    let ground_cap: f64 = nets.iter().map(|n| n.cap_ff).sum();
    let total_res: f64 = nets.iter().map(|n| n.res_ohm).sum();
    let total_coupling: f64 = couplings.iter().map(|c| c.cap_ff).sum::<f64>() + 0.0; // normalize -0.0
    let mut s = String::new();
    s.push_str(&format!("{{\"design\":{design:?},\"nets\":{},", nets.len()));
    s.push_str(&format!(
        "\"ground_cap_ff\":{ground_cap:.6},\"coupling_cap_ff\":{total_coupling:.6},\
         \"total_cap_ff\":{:.6},\"total_res_ohm\":{total_res:.6},",
        ground_cap + total_coupling
    ));
    s.push_str("\"per_net\":[");
    for (i, n) in nets.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"name\":{:?},\"pins\":{},\"ground_cap_ff\":{:.6},\"res_ohm\":{:.6}}}",
            n.name,
            n.pins.len(),
            n.cap_ff,
            n.res_ohm
        ));
    }
    s.push_str("],\"couplings\":[");
    for (i, c) in couplings.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("{{\"a\":{:?},\"b\":{:?},\"cap_ff\":{:.6}}}", c.a, c.b, c.cap_ff));
    }
    s.push_str("]}\n");
    s
}

/// Render a complete SPEF for the given net parasitics + coupling caps.
///
/// A coupling cap between nets A and B contributes to **both** nets' total
/// capacitance but is listed once (under net A) in the `*CAP` section as a
/// two-node entry, per IEEE 1481.
pub fn render(
    design: &str,
    units: &Units,
    date: Option<&str>,
    nets: &[NetParasitics],
    couplings: &[CouplingCap],
) -> String {
    // Name map: all net names first (so net ids are contiguous 1..N and coupling
    // references stay readable), then instance names.
    let mut nm = NameMap::new();
    let net_ids: Vec<usize> = nets.iter().map(|n| nm.intern(&n.name)).collect();
    let pin_ids: Vec<Vec<(usize, String)>> = nets
        .iter()
        .map(|net| net.pins.iter().map(|(inst, pin)| (nm.intern(inst), pin.clone())).collect())
        .collect();

    // Per-net coupling totals (both endpoints) + the list of couplings to emit
    // under each net (keyed by net A).
    let mut coupling_total: BTreeMap<String, f64> = BTreeMap::new();
    let mut under: BTreeMap<String, Vec<&CouplingCap>> = BTreeMap::new();
    for c in couplings {
        *coupling_total.entry(c.a.clone()).or_default() += c.cap_ff;
        *coupling_total.entry(c.b.clone()).or_default() += c.cap_ff;
        under.entry(c.a.clone()).or_default().push(c);
    }

    let mut s = String::new();
    s.push_str("*SPEF \"IEEE 1481-1999\"\n");
    s.push_str(&format!("*DESIGN \"{design}\"\n"));
    if let Some(d) = date {
        s.push_str(&format!("*DATE \"{d}\"\n"));
    }
    s.push_str("*VENDOR \"Vyges\"\n");
    s.push_str("*PROGRAM \"vyges-extract\"\n");
    s.push_str(&format!("*VERSION \"{}\"\n", crate::VERSION));
    s.push_str("*DIVIDER /\n");
    s.push_str("*DELIMITER :\n");
    s.push_str("*BUS_DELIMITER [ ]\n");
    s.push_str(&format!("*T_UNIT {}\n", units.time));
    s.push_str(&format!("*C_UNIT {}\n", units.cap));
    s.push_str(&format!("*R_UNIT {}\n", units.res));
    s.push_str("*L_UNIT 1 HENRY\n\n");

    s.push_str("*NAME_MAP\n");
    for (i, name) in nm.order.iter().enumerate() {
        s.push_str(&format!("*{} {}\n", i + 1, name));
    }
    s.push('\n');

    for (n, net) in nets.iter().enumerate() {
        let nid = net_ids[n];
        let cpl = coupling_total.get(&net.name).copied().unwrap_or(0.0);
        // total net cap = grounded + all coupling involving this net
        s.push_str(&format!("*D_NET *{} {}\n", nid, val(net.cap_ff + cpl)));

        s.push_str("*CONN\n");
        for (iid, pin) in &pin_ids[n] {
            // direction unknown without LEF in v0 -> default 'I'
            s.push_str(&format!("*I *{iid}:{pin} I\n"));
        }

        // Per-pin RC tree: a star rooted at the net node. The driver (pin 0) is
        // the near end — half the grounded C and the trunk R/2; the sinks share
        // the far half, each on its own branch (cap (C/2)/K, branch R/2K to the
        // root). This differentiates sinks (vs lumping the far C at one node)
        // and reduces to the pi for a single sink. Degrades to a 2-node pi for
        // one pin, and to a single lump with no series R. Uniform apportionment
        // (no per-pin geometry yet); coupling stays at the root (net node).
        let g = net.cap_ff;
        let r = net.res_ohm;
        let pins = &pin_ids[n];
        let mut caps: Vec<(String, f64)> = Vec::new(); // (node, grounded cap)
        let mut res_lines: Vec<(String, String, f64)> = Vec::new(); // (a, b, ohm)
        let node = |iid: usize, pin: &str| format!("{iid}:{pin}");

        if r <= 0.0 {
            caps.push((format!("{nid}"), g)); // single lump
        } else if pins.is_empty() {
            caps.push((format!("{nid}"), g / 2.0));
            caps.push((format!("{nid}:far"), g / 2.0));
            res_lines.push((format!("{nid}"), format!("{nid}:far"), r));
        } else {
            let (diid, dpin) = &pins[0]; // driver = pin 0
            let dnode = node(*diid, dpin);
            caps.push((dnode.clone(), g / 2.0)); // near half
            let sinks = &pins[1..];
            if sinks.is_empty() {
                caps.push((format!("{nid}"), g / 2.0)); // far half at root
                res_lines.push((format!("{nid}"), dnode, r));
            } else {
                res_lines.push((format!("{nid}"), dnode, r / 2.0)); // trunk
                let k = sinks.len() as f64;
                for (siid, spin) in sinks {
                    let sn = node(*siid, spin);
                    caps.push((sn.clone(), (g / 2.0) / k));
                    res_lines.push((format!("{nid}"), sn, r / 2.0 / k)); // branch
                }
            }
        }

        s.push_str("*CAP\n");
        let mut ci = 0;
        for (n_, c) in &caps {
            ci += 1;
            s.push_str(&format!("{ci} *{n_} {}\n", val(*c)));
        }
        // coupling caps listed under net A at the root (net node)
        if let Some(list) = under.get(&net.name) {
            for c in list {
                ci += 1;
                let bid = nm.id(&c.b).unwrap_or(nid);
                s.push_str(&format!("{ci} *{nid} *{bid} {}\n", val(c.cap_ff)));
            }
        }
        if !res_lines.is_empty() {
            s.push_str("*RES\n");
            for (i, (a, b, ohm)) in res_lines.iter().enumerate() {
                s.push_str(&format!("{} *{a} *{b} {}\n", i + 1, val(*ohm)));
            }
        }

        s.push_str("*END\n\n");
    }

    s
}
