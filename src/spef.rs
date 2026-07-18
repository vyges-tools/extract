//! SPEF (IEEE 1481) emitter.
//!
//! Emits a Standard Parasitic Exchange Format file with a name map and, per
//! net, a `*D_NET` record: `*CONN` (the connected instance pins), `*CAP` (the
//! grounded + coupling capacitance), and `*RES` (the series resistance).
//!
//! When a net's routing geometry yields a **distributed RC tree** (`tree.rs`),
//! the net is emitted with that tree's real internal wire-junction nodes and
//! per-segment / per-via resistors, scaled to the calibrated lumped totals (see
//! [`render_distributed`]). Without geometry it falls back to a lumped **star** —
//! a trunk R/2 + near-half C to the driver and a branch R/2K + cap share to each
//! sink, reducing to a pi for a single sink and a lump with no R.
//!
//! Pure std — fully unit-tested offline. No timestamp is embedded unless one is
//! passed in, so an unchanged run is bit-identical (the M2 reproducibility
//! contract).

use std::collections::BTreeMap;

use crate::coupling::CouplingCap;
use crate::rc::NetParasitics;
use crate::tree::RcNetwork;

#[derive(Debug, Clone)]
pub struct Units {
    pub time: String, // e.g. "1 PS"
    pub cap: String,  // e.g. "1 FF"
    pub res: String,  // e.g. "1 OHM"
}

impl Default for Units {
    fn default() -> Self {
        Units {
            time: "1 PS".into(),
            cap: "1 FF".into(),
            res: "1 OHM".into(),
        }
    }
}

/// Assigns stable integer ids to names in first-seen order (deterministic).
struct NameMap {
    ids: BTreeMap<String, usize>,
    order: Vec<String>,
}

impl NameMap {
    fn new() -> NameMap {
        NameMap {
            ids: BTreeMap::new(),
            order: Vec::new(),
        }
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

/// `(node label, grounded cap)` lines for a net's `*CAP` section.
type CapLines = Vec<(String, f64)>;
/// `(node a, node b, ohms)` lines for a net's `*RES` section.
type ResLines = Vec<(String, String, f64)>;

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
        s.push_str(&format!(
            "{{\"a\":{:?},\"b\":{:?},\"cap_ff\":{:.6}}}",
            c.a, c.b, c.cap_ff
        ));
    }
    s.push_str("]}\n");
    s
}

/// Render a complete SPEF using the lumped **star** topology for every net (the
/// fallback when no routing geometry is available). Equivalent to
/// [`render_distributed`] with all trees `None`.
pub fn render(
    design: &str,
    units: &Units,
    date: Option<&str>,
    nets: &[NetParasitics],
    couplings: &[CouplingCap],
) -> String {
    let none: Vec<Option<RcNetwork>> = (0..nets.len()).map(|_| None).collect();
    render_distributed(design, units, date, nets, &none, couplings, None)
}

/// Render a complete SPEF, emitting each net as a **distributed RC tree** when
/// `trees[i]` is present and a lumped star otherwise.
///
/// The distributed network (from `tree::build_network`) carries the real routing
/// topology — internal wire-junction nodes and per-segment / per-via resistors.
/// Its node caps and edge resistances are *scaled back* to the net's calibrated
/// lumped `cap_ff` / `res_ohm`, so magnitudes match the rule deck exactly while
/// the topology gains the detail delay/SI sign-off needs.
///
/// A coupling cap between nets A and B contributes to **both** nets' total
/// capacitance but is listed once (under net A) in the `*CAP` section as a
/// two-node entry, per IEEE 1481.
pub fn render_distributed(
    design: &str,
    units: &Units,
    date: Option<&str>,
    nets: &[NetParasitics],
    trees: &[Option<RcNetwork>],
    couplings: &[CouplingCap],
    resolver: Option<&crate::hookup::PinResolver>,
) -> String {
    // Name map: all net names first (so net ids are contiguous 1..N and coupling
    // references stay readable), then instance names.
    let mut nm = NameMap::new();
    let net_ids: Vec<usize> = nets.iter().map(|n| nm.intern(&n.name)).collect();
    let pin_ids: Vec<Vec<(usize, String)>> = nets
        .iter()
        .map(|net| {
            net.pins
                .iter()
                .map(|(inst, pin)| (nm.intern(inst), pin.clone()))
                .collect()
        })
        .collect();

    // The node each net presents to a coupling neighbour: the driver vertex when a
    // tree exists, else the net-id root of the star. Lets a coupling entry name a
    // real node on both sides regardless of each net's topology.
    let rep_label = |n: usize| -> String {
        match trees.get(n).and_then(|t| t.as_ref()) {
            Some(_) if !pin_ids[n].is_empty() => {
                let (iid, pin) = &pin_ids[n][0];
                format!("{iid}:{pin}")
            }
            Some(_) => format!("{}:0", net_ids[n]), // pinless tree -> first internal node
            None => format!("{}", net_ids[n]),      // star root
        }
    };
    let id_of: BTreeMap<&str, usize> = nets
        .iter()
        .enumerate()
        .map(|(i, n)| (n.name.as_str(), i))
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
        // std-cell hookup: per-pin (direction, load cap) from DEF+LEF+liberty.
        let hk: Vec<(vyges_loom::lef::PinDir, f64)> = net
            .pins
            .iter()
            .map(|(inst, pin)| {
                resolver
                    .map(|r| r.resolve(inst, pin))
                    .unwrap_or((vyges_loom::lef::PinDir::Unknown, 0.0))
            })
            .collect();
        let cin_sum: f64 = hk.iter().map(|(_, c)| *c).sum();
        // total net cap = grounded wire + coupling + per-load pin Cin
        s.push_str(&format!(
            "*D_NET *{} {}\n",
            nid,
            val(net.cap_ff + cpl + cin_sum)
        ));

        s.push_str("*CONN\n");
        for (k, (iid, pin)) in pin_ids[n].iter().enumerate() {
            let (dir, cap) = hk[k];
            let d = match dir {
                vyges_loom::lef::PinDir::Output => "O",
                vyges_loom::lef::PinDir::Inout => "B",
                _ => "I", // input / unknown → load
            };
            if cap > 0.0 {
                s.push_str(&format!("*I *{iid}:{pin} {d} *L {}\n", val(cap)));
            } else {
                s.push_str(&format!("*I *{iid}:{pin} {d}\n"));
            }
        }

        let (caps, res_lines) = match trees.get(n).and_then(|t| t.as_ref()) {
            Some(net_tree) => emit_tree(nid, net_tree, &nm, net.cap_ff, net.res_ohm),
            None => emit_star(nid, &pin_ids[n], net.cap_ff, net.res_ohm),
        };

        s.push_str("*CAP\n");
        let mut ci = 0;
        for (n_, c) in &caps {
            ci += 1;
            s.push_str(&format!("{ci} *{n_} {}\n", val(*c)));
        }
        // per-load pin Cin, grounded at the pin node (keeps Σ*CAP = *D_NET cap)
        for (k, (iid, pin)) in pin_ids[n].iter().enumerate() {
            let cap = hk[k].1;
            if cap > 0.0 {
                ci += 1;
                s.push_str(&format!("{ci} *{iid}:{pin} {}\n", val(cap)));
            }
        }
        // coupling caps listed under net A, between A's and B's representative nodes
        if let Some(list) = under.get(&net.name) {
            let rep_a = rep_label(n);
            for c in list {
                ci += 1;
                let rep_b = id_of
                    .get(c.b.as_str())
                    .map(|&j| rep_label(j))
                    .unwrap_or_else(|| {
                        nm.id(&c.b)
                            .map(|b| b.to_string())
                            .unwrap_or_else(|| nid.to_string())
                    });
                s.push_str(&format!("{ci} *{rep_a} *{rep_b} {}\n", val(c.cap_ff)));
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

/// Distributed emission: scale the geometric tree to the calibrated per-net
/// totals and render its nodes (`*CAP`) and edges (`*RES`). Pin nodes keep their
/// `inst:pin` label; internal junctions are `netid:nodeindex`.
fn emit_tree(
    nid: usize,
    t: &RcNetwork,
    nm: &NameMap,
    cap_ff: f64,
    res_ohm: f64,
) -> (CapLines, ResLines) {
    let scale_c = if t.raw_cap > 0.0 {
        cap_ff / t.raw_cap
    } else {
        0.0
    };
    let scale_r = if t.raw_res > 0.0 {
        res_ohm / t.raw_res
    } else {
        0.0
    };
    let label = |i: usize| -> String {
        match &t.nodes[i].pin {
            Some((inst, pin)) => format!("{}:{}", nm.id(inst).unwrap_or(0), pin),
            None => format!("{nid}:{i}"),
        }
    };
    let mut caps: Vec<(String, f64)> = Vec::new();
    for (i, node) in t.nodes.iter().enumerate() {
        let mut c = node.cap_ff * scale_c;
        // if the rule deck gives no grounded cap to distribute, lump the net total
        // at the first node so charge is never silently dropped
        if t.raw_cap <= 0.0 && i == 0 {
            c = cap_ff;
        }
        caps.push((label(i), c));
    }
    let res_lines: Vec<(String, String, f64)> = t
        .edges
        .iter()
        .map(|e| (label(e.a), label(e.b), e.res_ohm * scale_r))
        .collect();
    (caps, res_lines)
}

/// Lumped star emission (the geometry-free fallback): driver near-half + per-sink
/// far-half branches off the net-id root. Reduces to a pi for one sink and a lump
/// with no series R.
fn emit_star(nid: usize, pins: &[(usize, String)], g: f64, r: f64) -> (CapLines, ResLines) {
    let mut caps: Vec<(String, f64)> = Vec::new();
    let mut res_lines: Vec<(String, String, f64)> = Vec::new();
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
    (caps, res_lines)
}
