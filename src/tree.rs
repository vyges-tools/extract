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

/// A segment in integer nanometres, so "do these meet?" is an exact comparison
/// rather than a float tolerance.
#[derive(Clone)]
struct NmSeg {
    layer: String,
    a: (i64, i64),
    b: (i64, i64),
    width_um: f64,
}

/// Is `p` strictly inside the open segment `a`–`b` (collinear, not an endpoint)?
fn on_interior(a: (i64, i64), b: (i64, i64), p: (i64, i64)) -> bool {
    if p == a || p == b {
        return false;
    }
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (px, py) = (p.0 - a.0, p.1 - a.1);
    if dx * py - dy * px != 0 {
        return false; // not collinear
    }
    let dot = px * dx + py * dy;
    dot > 0 && dot < dx * dx + dy * dy
}

/// Split routing segments wherever another vertex of the same net lands **in the
/// middle** of one, so the two actually share a node.
///
/// Real DEF routing joins wires at points that are an endpoint of one and an interior
/// point of the other, in two flavours:
///
///   * **a via lands mid-span.** `NEW met1 ( 230690 1040230 ) M1M2_PR` connects to a met2
///     run from y=1035470 to y=1043460 — the via is 4.76 µm along it, an endpoint of
///     nothing. Nothing on met2 marks that point.
///   * **a same-layer T-junction**, where a branch taps a spine between its ends.
///
/// Interning only segment *endpoints* therefore leaves those wires as separate graphs.
/// That is what made 7 % of nets on a real block emit an RC network in more than one
/// piece, with sinks that had no resistive path to their driver at all.
///
/// The split points are chosen narrowly on purpose:
///
///   * **same-layer endpoints** — a T-junction is a connection by construction; and
///   * **declared via locations, on every layer** — because a via is DEF *stating* that a
///     connection exists at that point, and the layer it has to reach is precisely the one
///     with no vertex there.
///
/// It deliberately does **not** split at cross-layer endpoints generally. Two wires of one
/// net crossing on different layers with no via between them are *not* connected, and
/// giving them a shared location would let the via-stack pass below fabricate a resistor
/// across them — inventing connectivity is a worse failure than missing it.
///
/// Splitting is exact and conservative in magnitude: sub-segment lengths sum to the
/// original, and both the per-µm capacitance and `wire_res` are linear in length, so
/// `raw_cap`/`raw_res` are unchanged and the deck calibration is untouched.
fn split_at_junctions(segs: Vec<NmSeg>, via_points: &[(i64, i64)]) -> Vec<NmSeg> {
    use std::collections::{BTreeMap, BTreeSet};

    // Candidate split points per layer: every endpoint on that layer, plus every via
    // location regardless of the layer it was declared on.
    let mut by_layer: BTreeMap<String, BTreeSet<(i64, i64)>> = BTreeMap::new();
    for s in &segs {
        let e = by_layer.entry(s.layer.clone()).or_default();
        e.insert(s.a);
        e.insert(s.b);
    }
    for layer in by_layer.values_mut() {
        layer.extend(via_points.iter().copied());
    }

    let mut out = Vec::with_capacity(segs.len());
    for s in segs {
        let Some(points) = by_layer.get(s.layer.as_str()) else {
            out.push(s);
            continue;
        };
        let mut cuts: Vec<(i64, i64)> = points
            .iter()
            .copied()
            .filter(|&p| on_interior(s.a, s.b, p))
            .collect();
        if cuts.is_empty() {
            out.push(s);
            continue;
        }
        // Order the cuts along the segment, then walk it end to end.
        let (dx, dy) = (s.b.0 - s.a.0, s.b.1 - s.a.1);
        cuts.sort_by_key(|p| (p.0 - s.a.0) * dx + (p.1 - s.a.1) * dy);
        let mut from = s.a;
        for c in cuts.into_iter().chain(std::iter::once(s.b)) {
            out.push(NmSeg {
                layer: s.layer.clone(),
                a: from,
                b: c,
                width_um: s.width_um,
            });
            from = c;
        }
    }
    out
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

/// How many connected components the `*RES` graph has, over all nodes.
///
/// Also the exact check OpenRCX applies when it reports `RCX-0272 RC of net … is
/// disconnected`, so a net that passes here passes there.
pub fn components(nodes: &[RcNode], edges: &[RcEdge]) -> usize {
    let mut parent: Vec<usize> = (0..nodes.len()).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for e in edges {
        let (ra, rb) = (find(&mut parent, e.a), find(&mut parent, e.b));
        if ra != rb {
            parent[ra] = rb;
        }
    }
    (0..nodes.len())
        .filter(|&i| find(&mut parent, i) == i)
        .count()
}

/// What came of trying to build a distributed network for one net.
///
/// The caller falls back to the lumped star for anything but `Built`. The two failure
/// cases are kept apart on purpose: `NoGeometry` is ordinary and expected, while
/// `Disconnected` means geometry was present and we could not make one network of it —
/// a defect, and one worth counting rather than quietly absorbing into the same bucket.
#[derive(Debug, Clone)]
pub enum Outcome {
    Built(RcNetwork),
    /// No usable geometry: no segments, a degenerate point, a layer missing from the
    /// rules, or more pins than vertices to place them on.
    NoGeometry,
    /// Geometry was present, but the network came out in more than one piece.
    Disconnected {
        pieces: usize,
    },
}

impl Outcome {
    /// The network, if one was built.
    pub fn built(self) -> Option<RcNetwork> {
        match self {
            Outcome::Built(t) => Some(t),
            _ => None,
        }
    }
}

/// Build the distributed RC network for one net from its routing geometry.
pub fn build_network(
    net: &DefNet,
    rules: &RcRules,
    widths: &BTreeMap<String, f64>,
    via_layers: &BTreeMap<String, Vec<String>>,
) -> Outcome {
    if net.segments.is_empty() {
        return Outcome::NoGeometry;
    }

    // (vertex location, layer) -> node index. The location collapses coincident
    // endpoints; splitting by layer at a shared location lets a via stack there
    // become an explicit resistor between the per-layer sub-nodes.
    let mut sub: BTreeMap<(i64, i64, String), usize> = BTreeMap::new();
    // layers present at each geometric location, for via reconstruction
    let mut at_loc: BTreeMap<(i64, i64), Vec<String>> = BTreeMap::new();
    let mut nodes: Vec<RcNode> = Vec::new();
    let mut edges: Vec<RcEdge> = Vec::new();

    // Work in integer nanometres from here so "these two meet" is exact, and split every
    // segment at the junctions that land inside it before any node is interned — see
    // `split_at_junctions` for why endpoint-only interning is not enough.
    let nm: Vec<NmSeg> = net
        .segments
        .iter()
        .map(|s| NmSeg {
            layer: s.layer.clone(),
            a: (qkey(s.x0), qkey(s.y0)),
            b: (qkey(s.x1), qkey(s.y1)),
            width_um: s.width_um,
        })
        .collect();
    let via_pts: Vec<(i64, i64)> = net
        .via_points
        .iter()
        .map(|v| (qkey(v.x), qkey(v.y)))
        .collect();

    for seg in split_at_junctions(nm, &via_pts) {
        let Some(l) = rules.layer(&seg.layer) else {
            return Outcome::NoGeometry; // unknown layer -> lumped path
        };
        let a = node_of(&mut sub, &mut at_loc, &mut nodes, seg.a, &seg.layer);
        let b = node_of(&mut sub, &mut at_loc, &mut nodes, seg.b, &seg.layer);
        // Manhattan length, in microns, from the quantized endpoints.
        let len = ((seg.b.0 - seg.a.0).abs() + (seg.b.1 - seg.a.1).abs()) as f64 / 1000.0;
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

    // A via's own layer may carry no wire at all in this net — `NEW li1 ( 232990 1034790 )
    // L1M1_PR_MR` is a bare landing, and it is how a pin reaches the route. Give it a node so
    // the stack has something to connect to, but only where the location already has another
    // layer: otherwise the node would be isolated, and an isolated node is a `*CAP` entry no
    // `*RES` edge reaches — the very defect this is fixing.
    for (v, layer) in via_pts.iter().zip(net.via_points.iter().map(|v| &v.layer)) {
        if at_loc.contains_key(v) && rules.layer(layer).is_some() {
            node_of(&mut sub, &mut at_loc, &mut nodes, *v, layer);
        }
    }

    // Via resistors, between the per-layer sub-nodes at a location. Total via resistance is
    // scaled to the net's reported via count at emit time; here each transition carries one
    // via_res.
    //
    // Which pairs get one is the delicate part. "Every layer present at this location" was
    // safe only while nodes existed at segment endpoints alone. Splitting puts nodes at far
    // more places — including where *another* layer's via cut through — so that rule now
    // fabricates connections: a met2→met3 via at a point the met1 wire merely passes through
    // would also short met1 to met2, and if those two are legitimately joined elsewhere the
    // net comes back with a loop in it. That is how the loop count went up when the
    // disconnection count went to zero.
    //
    // So gate on what the routing DECLARES. DEF names a via on the lower of the two layers it
    // joins (`li1 … L1M1_PR`, `met1 … M1M2_PR`, `met2 … M2M3_PR`), so a pair is connected only
    // where a via was declared on the lower one. A node no via accounts for stays what it
    // physically is — a Steiner point on its own wire.
    let mut declared: BTreeMap<(i64, i64), Vec<&crate::def::ViaPoint>> = BTreeMap::new();
    for (v, vp) in via_pts.iter().zip(net.via_points.iter()) {
        declared.entry(*v).or_default().push(vp);
    }
    let present = |loc: &(i64, i64), l: &str| sub.contains_key(&(loc.0, loc.1, l.to_string()));

    if net.via_points.is_empty() {
        // A source that gives no via placements at all — the GDS tracer establishes
        // connectivity by shape overlap and returns only counts — keeps the old co-location
        // rule. Gating on declarations we were never given would disconnect everything.
        for (loc, layers) in &at_loc {
            let mut ls = layers.clone();
            ls.sort();
            for w in ls.windows(2) {
                edges.push(RcEdge {
                    a: sub[&(loc.0, loc.1, w[0].clone())],
                    b: sub[&(loc.0, loc.1, w[1].clone())],
                    res_ohm: rules.via_res.max(0.0),
                });
            }
        }
    } else {
        for (loc, vps) in &declared {
            for vp in vps {
                // 1. The LEF, which is the only authority: its VIA block names the layers.
                let from_lef = via_layers.get(&vp.name).map(|ls| {
                    ls.iter()
                        .filter(|l| present(loc, l))
                        .cloned()
                        .collect::<Vec<_>>()
                });
                // 2. Failing that (no LEF, or a via it does not define), the declared layer
                //    plus the one adjacent layer present here. Where a layer sits both above
                //    and below, that is genuinely ambiguous and we do not guess — the net
                //    then fails the connectivity check and degrades to a star, counted and
                //    visible, rather than being wired up wrong and looking fine.
                let pair = match from_lef {
                    Some(ls) if ls.len() == 2 => Some((ls[0].clone(), ls[1].clone())),
                    _ => {
                        let mut here = at_loc.get(loc).cloned().unwrap_or_default();
                        here.sort();
                        match here.iter().position(|l| *l == vp.layer) {
                            None => None,
                            Some(i) => {
                                let below = i.checked_sub(1).map(|j| here[j].clone());
                                let above = here.get(i + 1).cloned();
                                match (below, above) {
                                    (Some(b), None) => Some((b, vp.layer.clone())),
                                    (None, Some(a)) => Some((vp.layer.clone(), a)),
                                    _ => None,
                                }
                            }
                        }
                    }
                };
                if let Some((a, b)) = pair {
                    if present(loc, &a) && present(loc, &b) {
                        edges.push(RcEdge {
                            a: sub[&(loc.0, loc.1, a)],
                            b: sub[&(loc.0, loc.1, b)],
                            res_ohm: rules.via_res.max(0.0),
                        });
                    }
                }
            }
        }
    }

    if nodes.len() < 2 || edges.is_empty() {
        return Outcome::NoGeometry; // nothing distributed to say -> lumped star
    }

    // The network has to be ONE network. A SPEF whose `*RES` edges leave some `*CAP` nodes
    // unreachable describes a net with no resistive path from its driver to some of its
    // sinks — a timer reading it sees no interconnect delay to those pins at all. Splitting
    // above fixes the cause we know about; this is the backstop that keeps the invariant
    // true unconditionally, because emitting a coarse but valid star beats emitting a
    // detailed invalid tree. The count is reported, not swallowed.
    let pieces = components(&nodes, &edges);
    if pieces > 1 {
        return Outcome::Disconnected { pieces };
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
        return Outcome::NoGeometry; // can't place every pin on a distinct vertex -> lumped
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
    Outcome::Built(RcNetwork {
        nodes,
        edges,
        raw_cap,
        raw_res,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::{Segment, ViaPoint};

    fn seg(layer: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> Segment {
        Segment::wire(layer, x0, y0, x1, y1)
    }

    #[test]
    fn two_pin_wire_is_two_nodes_one_resistor() {
        let net = DefNet {
            name: "n".into(),
            raw_name: String::new(),
            pins: vec![("u0".into(), "Y".into()), ("u1".into(), "A".into())],
            segments: vec![seg("met1", 0.0, 0.0, 10.0, 0.0)],
            vias: 0,
            via_points: Vec::new(),
        };
        let rules = RcRules::parse("met1 0.1 0.05 0.0\n").unwrap();
        let t = build_network(&net, &rules, &BTreeMap::new(), &BTreeMap::new())
            .built()
            .unwrap();
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
            raw_name: String::new(),
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
            via_points: Vec::new(),
        };
        let rules = RcRules::parse("met1 0.1 0.05 0.0\n").unwrap();
        let t = build_network(&net, &rules, &BTreeMap::new(), &BTreeMap::new())
            .built()
            .unwrap();
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

    /// Real geometry from `_00768_` of an fft control block — the net that started this.
    ///
    /// The `M1M2_PR` via at (230.690, 1040.230) sits **mid-span** of the met2 run from
    /// y=1035.470 to y=1043.460. Interning only endpoints leaves the met1 branch to the west
    /// as its own graph, with no resistive path to anything.
    #[test]
    fn a_via_landing_mid_span_still_joins_the_layers() {
        let net = DefNet {
            name: "n".into(),
            raw_name: String::new(),
            pins: vec![("a".into(), "A".into()), ("b".into(), "Y".into())],
            segments: vec![
                seg("met2", 230.690, 1035.470, 230.690, 1043.460), // the run it lands on
                seg("met1", 226.550, 1040.230, 230.690, 1040.230), // the branch it feeds
            ],
            vias: 1,
            via_points: vec![ViaPoint {
                x: 230.690,
                y: 1040.230,
                layer: "met1".into(),
                name: "M1M2_PR".into(),
            }],
        };
        let rules = RcRules::parse("met1 0.1 0.05 0.0\nmet2 0.1 0.05 0.0\nvia 5.0\n").unwrap();
        let t = build_network(&net, &rules, &BTreeMap::new(), &BTreeMap::new())
            .built()
            .expect("one network");
        assert_eq!(components(&t.nodes, &t.edges), 1, "one net, one network");
        // the met2 run was cut in two at the via, so its resistance is unchanged in total
        assert!(
            (t.raw_res - (0.799 + 0.414 + 5.0)).abs() < 1e-6,
            "7.99um + 4.14um of wire at 0.1 ohm/um, plus the via: got {}",
            t.raw_res
        );
    }

    #[test]
    fn a_same_layer_t_junction_joins() {
        // a branch taps a spine between its ends — an endpoint of one, interior of the other
        let net = DefNet {
            name: "n".into(),
            raw_name: String::new(),
            pins: vec![("a".into(), "A".into()), ("b".into(), "Y".into())],
            segments: vec![
                seg("met1", 0.0, 0.0, 20.0, 0.0),  // spine
                seg("met1", 10.0, 0.0, 10.0, 5.0), // taps it at x=10, mid-span
            ],
            vias: 0,
            via_points: Vec::new(),
        };
        let rules = RcRules::parse("met1 0.1 0.05 0.0\n").unwrap();
        let t = build_network(&net, &rules, &BTreeMap::new(), &BTreeMap::new())
            .built()
            .expect("one network");
        assert_eq!(components(&t.nodes, &t.edges), 1);
        assert_eq!(t.nodes.len(), 4, "spine ends + the tap point + branch end");
        assert_eq!(t.edges.len(), 3, "the spine is cut at the tap");
    }

    /// The line we deliberately do not cross.
    ///
    /// Two wires of one net on different layers, crossing with **no via** between them, are
    /// not connected. Splitting them at the crossing would give them a shared location, and
    /// the via-stack pass would then fabricate a resistor across it — inventing connectivity
    /// that the layout does not have. Missing a connection is recoverable; inventing one
    /// silently changes the RC of a net that looks fine.
    #[test]
    fn crossing_layers_without_a_via_are_not_shorted() {
        let net = DefNet {
            name: "n".into(),
            raw_name: String::new(),
            pins: vec![("a".into(), "A".into()), ("b".into(), "Y".into())],
            segments: vec![
                seg("met1", 0.0, 5.0, 20.0, 5.0),   // horizontal on met1
                seg("met2", 10.0, 0.0, 10.0, 10.0), // vertical on met2, crossing at (10,5)
            ],
            vias: 0,
            via_points: Vec::new(),
        };
        let rules = RcRules::parse("met1 0.1 0.05 0.0\nmet2 0.1 0.05 0.0\nvia 5.0\n").unwrap();
        // Genuinely two pieces, so the builder refuses and the caller uses the lumped star.
        match build_network(&net, &rules, &BTreeMap::new(), &BTreeMap::new()) {
            Outcome::Disconnected { pieces } => assert_eq!(pieces, 2),
            other => panic!("expected two disconnected pieces, got {other:?}"),
        }
    }

    /// The other half of "do not invent connectivity", and the one splitting created.
    ///
    /// Real shape from `_00812_`. A met2→met3 via sits at a point the met1 wire merely runs
    /// through. Splitting rightly puts a met1 node there — but if every layer present at a
    /// location is then chained, met1 gets shorted to met2 at that point *as well as* at the
    /// M1M2 via further along, and the net comes back with a loop in it.
    #[test]
    fn a_via_does_not_short_a_layer_that_merely_passes_through() {
        let net = DefNet {
            name: "n".into(),
            raw_name: String::new(),
            pins: vec![("a".into(), "A".into()), ("b".into(), "Y".into())],
            segments: vec![
                seg("met1", 630.430, 1371.730, 630.430, 1372.070), // runs past 1371.900
                seg("met2", 630.430, 1371.900, 630.430, 1372.070),
                seg("met3", 630.430, 1371.900, 630.660, 1371.900),
            ],
            vias: 2,
            via_points: vec![
                ViaPoint {
                    x: 630.430,
                    y: 1372.070,
                    layer: "met1".into(), // M1M2 — the genuine met1/met2 join
                    name: "M1M2_PR".into(),
                },
                ViaPoint {
                    x: 630.430,
                    y: 1371.900,
                    layer: "met2".into(), // M2M3 — says nothing about met1
                    name: "M2M3_PR".into(),
                },
            ],
        };
        let rules =
            RcRules::parse("met1 0.1 0.05 0.0\nmet2 0.1 0.05 0.0\nmet3 0.1 0.05 0.0\nvia 5.0\n")
                .unwrap();
        // What the tech LEF says these vias join — the only authority on the question.
        let via_layers: BTreeMap<String, Vec<String>> = [
            ("M1M2_PR", vec!["via", "met1", "met2"]),
            ("M2M3_PR", vec!["via2", "met2", "met3"]),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.into_iter().map(String::from).collect()))
        .collect();

        let t = build_network(&net, &rules, &BTreeMap::new(), &via_layers)
            .built()
            .expect("one network");
        assert_eq!(components(&t.nodes, &t.edges), 1, "still one network");
        assert_eq!(
            t.edges.len(),
            t.nodes.len() - 1,
            "and a TREE: {} nodes, {} edges — a loop means a via was invented",
            t.nodes.len(),
            t.edges.len()
        );
    }

    /// And when nothing can settle the question, we decline rather than guess.
    ///
    /// Same shape, no LEF. The via is declared on met2 with met1 below and met3 above, so
    /// which pair it joins is genuinely unknown. Wiring up either would be a coin flip that
    /// produces a confident-looking SPEF, so the net degrades to the lumped star instead and
    /// the caller counts it.
    #[test]
    fn an_ambiguous_via_with_no_lef_degrades_rather_than_guesses() {
        let net = DefNet {
            name: "n".into(),
            raw_name: String::new(),
            pins: vec![("a".into(), "A".into()), ("b".into(), "Y".into())],
            segments: vec![
                seg("met1", 630.430, 1371.730, 630.430, 1372.070),
                seg("met2", 630.430, 1371.900, 630.430, 1372.070),
                seg("met3", 630.430, 1371.900, 630.660, 1371.900),
            ],
            vias: 2,
            via_points: vec![
                ViaPoint {
                    x: 630.430,
                    y: 1372.070,
                    layer: "met1".into(),
                    name: "M1M2_PR".into(),
                },
                ViaPoint {
                    x: 630.430,
                    y: 1371.900,
                    layer: "met2".into(),
                    name: "M2M3_PR".into(),
                },
            ],
        };
        let rules =
            RcRules::parse("met1 0.1 0.05 0.0\nmet2 0.1 0.05 0.0\nmet3 0.1 0.05 0.0\nvia 5.0\n")
                .unwrap();
        assert!(
            matches!(
                build_network(&net, &rules, &BTreeMap::new(), &BTreeMap::new()),
                Outcome::Disconnected { .. }
            ),
            "no LEF, three layers at one point: refuse, do not invent"
        );
    }

    /// With only two layers at the point there is nothing to be ambiguous about, so the
    /// LEF is not needed — and the via may be declared on EITHER side of the pair, which is
    /// why the declared layer alone cannot be assumed to be the lower one.
    #[test]
    fn an_unambiguous_via_needs_no_lef_and_ignores_which_side_declared_it() {
        for declared_on in ["met1", "met2"] {
            let net = DefNet {
                name: "n".into(),
                raw_name: String::new(),
                pins: vec![("a".into(), "A".into()), ("b".into(), "Y".into())],
                segments: vec![
                    seg("met1", 1.0, 2.0, 1.0, 5.0),
                    seg("met2", 1.0, 5.0, 4.0, 5.0),
                ],
                vias: 1,
                via_points: vec![ViaPoint {
                    x: 1.0,
                    y: 5.0,
                    layer: declared_on.into(),
                    name: "M1M2_VIA".into(),
                }],
            };
            let rules = RcRules::parse("met1 0.1 0.05 0.0\nmet2 0.1 0.05 0.0\nvia 5.0\n").unwrap();
            let t = build_network(&net, &rules, &BTreeMap::new(), &BTreeMap::new())
                .built()
                .unwrap_or_else(|| panic!("declared on {declared_on}: expected one network"));
            assert_eq!(
                components(&t.nodes, &t.edges),
                1,
                "declared on {declared_on}"
            );
        }
    }

    #[test]
    fn splitting_conserves_the_totals_the_deck_calibrated() {
        // Same wire, once whole and once tapped in the middle. The tap adds a node and an
        // edge; it must not add or lose a femtofarad or an ohm, or the deck calibration
        // silently drifts with the routing's shape.
        let rules = RcRules::parse("met1 0.1 0.05 0.0\n").unwrap();
        let whole = DefNet {
            name: "n".into(),
            raw_name: String::new(),
            pins: vec![("a".into(), "A".into()), ("b".into(), "Y".into())],
            segments: vec![seg("met1", 0.0, 0.0, 20.0, 0.0)],
            vias: 0,
            via_points: Vec::new(),
        };
        let mut tapped = whole.clone();
        tapped.segments.push(seg("met1", 10.0, 0.0, 10.0, 0.001));
        let a = build_network(&whole, &rules, &BTreeMap::new(), &BTreeMap::new())
            .built()
            .unwrap();
        let b = build_network(&tapped, &rules, &BTreeMap::new(), &BTreeMap::new())
            .built()
            .unwrap();
        let stub_c = 0.001 * 0.05;
        let stub_r = 0.001 * 0.1;
        assert!(
            (b.raw_cap - a.raw_cap - stub_c).abs() < 1e-9,
            "capacitance is the original plus only the stub: {} vs {}",
            b.raw_cap,
            a.raw_cap
        );
        assert!(
            (b.raw_res - a.raw_res - stub_r).abs() < 1e-9,
            "resistance likewise: {} vs {}",
            b.raw_res,
            a.raw_res
        );
    }

    #[test]
    fn via_transition_adds_a_resistor() {
        // met1 then met2 meeting at (10,0): a via resistor links the two layers.
        let net = DefNet {
            name: "n".into(),
            raw_name: String::new(),
            pins: vec![("u0".into(), "Y".into()), ("u1".into(), "A".into())],
            segments: vec![
                seg("met1", 0.0, 0.0, 10.0, 0.0),
                seg("met2", 10.0, 0.0, 10.0, 8.0),
            ],
            vias: 1,
            via_points: Vec::new(),
        };
        let rules = RcRules::parse("met1 0.1 0.05 0.0\nmet2 0.1 0.05 0.0\nvia 5.0\n").unwrap();
        let t = build_network(&net, &rules, &BTreeMap::new(), &BTreeMap::new())
            .built()
            .unwrap();
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
