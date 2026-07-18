//! Distributed RC tree from routing geometry.
//!
//! The lumped path (`rc.rs`) gives one R and one grounded C per net; the SPEF
//! emitter then spreads them over a synthetic *star* (a fixed R/2 trunk + R/2K
//! branches). That ignores where the wire actually forks, so a sink hidden behind
//! a long resistive spine looks as close as one right next to the driver.
//!
//! This module recovers the **real topology** from the DEF routing: every routing
//! vertex (a coordinate where segments meet, including a via landing) becomes a
//! node, every wire segment becomes a resistor between two nodes with its
//! capacitance split to its endpoints, and a via stack at a vertex becomes a
//! resistor between the per-layer sub-nodes there. The result is a distributed RC
//! network with genuine internal wire-junction nodes — what delay/SI sign-off
//! needs — instead of a star.
//!
//! Magnitudes stay calibrated: the network is a *redistribution* of the same
//! per-net R and C the rule deck already produces (the emitter scales node caps
//! and edge resistances so they sum back to the lumped totals), so this adds
//! topology without disturbing the existing correlation.
//!
//! Bound (honest): the signal DEF gives segment geometry but not pin-access
//! coordinates, so pins are bound to the tree's terminal (leaf) vertices in a
//! deterministic order rather than by exact location — the remaining refinement
//! is to read pin access points from LEF/DEF `PINS`. Pure std, unit-tested.

use std::collections::BTreeMap;

use crate::def::DefNet;
use crate::rules::RcRules;

/// One node of the distributed network. A node is either a routing vertex that a
/// pin attaches to, or an internal wire junction / via landing.
#[derive(Debug, Clone)]
pub struct RcNode {
    /// Raw grounded capacitance at this node (pre-scale, fF).
    pub cap_ff: f64,
    /// The pin bound here, if any (`(instance, pin)`); `None` = internal node.
    pub pin: Option<(String, String)>,
}

/// A resistor between two nodes (wire segment or via).
#[derive(Debug, Clone)]
pub struct RcEdge {
    pub a: usize,
    pub b: usize,
    pub res_ohm: f64,
}

/// A per-net distributed RC network. `raw_cap`/`raw_res` are the geometric sums
/// the emitter scales back to the calibrated per-net totals.
#[derive(Debug, Clone)]
pub struct RcNetwork {
    pub nodes: Vec<RcNode>,
    pub edges: Vec<RcEdge>,
    pub raw_cap: f64,
    pub raw_res: f64,
}

/// Quantize a micron coordinate to an integer nanometre key so coincident
/// endpoints (the same physical vertex) collapse to one node.
fn qkey(v: f64) -> i64 {
    (v * 1000.0).round() as i64
}

/// Intern the (location, layer) sub-node, recording the layers present at the
/// location for later via reconstruction.
fn node_of(
    sub: &mut BTreeMap<(i64, i64, String), usize>,
    at_loc: &mut BTreeMap<(i64, i64), Vec<String>>,
    nodes: &mut Vec<RcNode>,
    loc: (i64, i64),
    layer: &str,
) -> usize {
    let key = (loc.0, loc.1, layer.to_string());
    if let Some(&i) = sub.get(&key) {
        return i;
    }
    let i = nodes.len();
    nodes.push(RcNode {
        cap_ff: 0.0,
        pin: None,
    });
    sub.insert(key, i);
    let layers = at_loc.entry(loc).or_default();
    if !layers.iter().any(|l| l == layer) {
        layers.push(layer.to_string());
    }
    i
}

/// Build the distributed RC network for one net from its routing geometry.
///
/// Returns `None` when there is no usable tree to build (no segments, a single
/// degenerate point, or a layer missing from the rules) — the caller then falls
/// back to the lumped star, so behaviour never regresses.
pub fn build_network(
    net: &DefNet,
    rules: &RcRules,
    widths: &BTreeMap<String, f64>,
) -> Option<RcNetwork> {
    if net.segments.is_empty() {
        return None;
    }

    // (vertex location, layer) -> node index. The location collapses coincident
    // endpoints; splitting by layer at a shared location lets a via stack there
    // become an explicit resistor between the per-layer sub-nodes.
    let mut sub: BTreeMap<(i64, i64, String), usize> = BTreeMap::new();
    // layers present at each geometric location, for via reconstruction
    let mut at_loc: BTreeMap<(i64, i64), Vec<String>> = BTreeMap::new();
    let mut nodes: Vec<RcNode> = Vec::new();
    let mut edges: Vec<RcEdge> = Vec::new();

    for seg in &net.segments {
        let l = rules.layer(&seg.layer)?; // unknown layer -> bail to lumped path
        let p0 = (qkey(seg.x0), qkey(seg.y0));
        let p1 = (qkey(seg.x1), qkey(seg.y1));
        let a = node_of(&mut sub, &mut at_loc, &mut nodes, p0, &seg.layer);
        let b = node_of(&mut sub, &mut at_loc, &mut nodes, p1, &seg.layer);
        let len = seg.len_um();
        let half_c = len * l.cap_per_um / 2.0;
        nodes[a].cap_ff += half_c;
        nodes[b].cap_ff += half_c;
        if a != b {
            let w = if seg.width_um > 0.0 {
                seg.width_um
            } else {
                widths.get(&seg.layer).copied().unwrap_or(0.0)
            };
            let res_ohm = rules
                .wire_res(&seg.layer, len, w)
                .unwrap_or(len * l.res_per_um);
            edges.push(RcEdge { a, b, res_ohm });
        }
    }

    // Via resistors: at any location where the route changes layer, connect the
    // per-layer sub-nodes in a stack. Total via resistance is scaled to the net's
    // reported via count at emit time; here each transition carries one via_res.
    for (loc, layers) in &at_loc {
        if layers.len() < 2 {
            continue;
        }
        let mut ls = layers.clone();
        ls.sort();
        for w in ls.windows(2) {
            let a = sub[&(loc.0, loc.1, w[0].clone())];
            let b = sub[&(loc.0, loc.1, w[1].clone())];
            edges.push(RcEdge {
                a,
                b,
                res_ohm: rules.via_res.max(0.0),
            });
        }
    }

    if nodes.len() < 2 || edges.is_empty() {
        return None; // nothing distributed to say -> lumped star
    }

    // Bind pins to vertices: drivers/sinks live at the tree's terminals (degree-1
    // leaves). Assign in pin order to leaves in node order (deterministic); spill
    // extra pins onto the highest-degree junctions (a pin tapping a Steiner point).
    let mut degree = vec![0usize; nodes.len()];
    for e in &edges {
        degree[e.a] += 1;
        degree[e.b] += 1;
    }
    let leaves: Vec<usize> = (0..nodes.len()).filter(|&i| degree[i] == 1).collect();
    let mut spill: Vec<usize> = (0..nodes.len()).filter(|&i| degree[i] != 1).collect();
    spill.sort_by(|&x, &y| degree[y].cmp(&degree[x]).then(x.cmp(&y)));
    if net.pins.len() > nodes.len() {
        return None; // can't place every pin on a distinct vertex -> lumped
    }
    let mut leaf_i = 0;
    let mut spill_i = 0;
    for (inst, pin) in &net.pins {
        let target = if leaf_i < leaves.len() {
            let t = leaves[leaf_i];
            leaf_i += 1;
            t
        } else {
            let t = spill[spill_i];
            spill_i += 1;
            t
        };
        nodes[target].pin = Some((inst.clone(), pin.clone()));
    }

    let raw_cap: f64 = nodes.iter().map(|n| n.cap_ff).sum();
    let raw_res: f64 = edges.iter().map(|e| e.res_ohm).sum();
    Some(RcNetwork {
        nodes,
        edges,
        raw_cap,
        raw_res,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::Segment;

    fn seg(layer: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> Segment {
        Segment::wire(layer, x0, y0, x1, y1)
    }

    #[test]
    fn two_pin_wire_is_two_nodes_one_resistor() {
        let net = DefNet {
            name: "n".into(),
            pins: vec![("u0".into(), "Y".into()), ("u1".into(), "A".into())],
            segments: vec![seg("met1", 0.0, 0.0, 10.0, 0.0)],
            vias: 0,
        };
        let rules = RcRules::parse("met1 0.1 0.05 0.0\n").unwrap();
        let t = build_network(&net, &rules, &BTreeMap::new()).unwrap();
        assert_eq!(t.nodes.len(), 2);
        assert_eq!(t.edges.len(), 1);
        assert!((t.raw_res - 1.0).abs() < 1e-9, "10um * 0.1 = 1.0"); // wire R
        assert!((t.raw_cap - 0.5).abs() < 1e-9, "10um * 0.05 = 0.5"); // grounded C
                                                                      // each pin landed on its own endpoint
        assert!(t.nodes.iter().filter(|n| n.pin.is_some()).count() == 2);
    }

    #[test]
    fn forked_route_has_internal_junction() {
        // a driver spine (0..10) that forks at x=10 to two sinks -> the fork is a
        // genuine internal node, not collapsed into a star.
        let net = DefNet {
            name: "n".into(),
            pins: vec![
                ("drv".into(), "Y".into()),
                ("s0".into(), "A".into()),
                ("s1".into(), "A".into()),
            ],
            segments: vec![
                seg("met1", 0.0, 0.0, 10.0, 0.0),   // spine
                seg("met1", 10.0, 0.0, 10.0, 5.0),  // branch up
                seg("met1", 10.0, 0.0, 10.0, -5.0), // branch down
            ],
            vias: 0,
        };
        let rules = RcRules::parse("met1 0.1 0.05 0.0\n").unwrap();
        let t = build_network(&net, &rules, &BTreeMap::new()).unwrap();
        assert_eq!(t.nodes.len(), 4, "spine end + fork + 2 sink ends");
        assert_eq!(t.edges.len(), 3);
        // the fork vertex (degree 3) carries no pin -> an internal junction
        let mut degree = vec![0usize; t.nodes.len()];
        for e in &t.edges {
            degree[e.a] += 1;
            degree[e.b] += 1;
        }
        let fork = degree
            .iter()
            .position(|&d| d == 3)
            .expect("a degree-3 junction exists");
        assert!(t.nodes[fork].pin.is_none(), "the junction is internal");
    }

    #[test]
    fn via_transition_adds_a_resistor() {
        // met1 then met2 meeting at (10,0): a via resistor links the two layers.
        let net = DefNet {
            name: "n".into(),
            pins: vec![("u0".into(), "Y".into()), ("u1".into(), "A".into())],
            segments: vec![
                seg("met1", 0.0, 0.0, 10.0, 0.0),
                seg("met2", 10.0, 0.0, 10.0, 8.0),
            ],
            vias: 1,
        };
        let rules = RcRules::parse("met1 0.1 0.05 0.0\nmet2 0.1 0.05 0.0\nvia 5.0\n").unwrap();
        let t = build_network(&net, &rules, &BTreeMap::new()).unwrap();
        // 3 sub-nodes (met1@0, shared@10 split into met1/met2, met2@8) -> 4 nodes
        assert_eq!(t.nodes.len(), 4);
        // 2 wire resistors + 1 via resistor
        assert_eq!(t.edges.len(), 3);
        assert!(
            t.edges.iter().any(|e| (e.res_ohm - 5.0).abs() < 1e-9),
            "via resistor present"
        );
    }
}
