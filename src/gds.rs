//! `gds → DefNet` connectivity-tracing front-end (a GDS-only analog flow).
//!
//! The RC core consumes `&[DefNet]` (per-net layer-tagged Manhattan segments +
//! via count + pins) and is **format-agnostic** — it does not care whether those
//! nets came from a routed DEF or from raw GDS geometry. The usual analog path
//! is **DEF** (`examples/bias_gen/`, validated in `tests/analog.rs`): if a router
//! emitted a routed DEF, extraction works today, unchanged. This module is the
//! *other* on-ramp: when only a **GDS** layout exists, trace its connected wire
//! geometry into the same `DefNet` view, so the unchanged RC core can extract it.
//!
//! It sits strictly **above** `rc.rs`: it produces `DefNet`s and hands them to
//! the existing engine. The RC math (`rc.rs`, `coupling.rs`) is untouched.
//!
//! ## Algorithm
//!
//! 1. **Flatten** the GDS to one cell (instances expanded), via [`vyges_layout`].
//! 2. **Classify** each axis-aligned rectangle by a small **layer map**
//!    (`GDS layer/datatype → role`): a *routing* layer (named, e.g. `met1`) or a
//!    *via/cut* layer that joins two routing layers.
//! 3. **Trace connectivity** with union-find: two routing rects on the *same*
//!    layer join if they touch; a routing rect joins a routing rect on *another*
//!    layer only where a **via-layer rect overlaps both** (contact-gated, like
//!    `vyges-lvs` — abutment across layers does not short).
//! 4. **Reduce** each connected component to a [`DefNet`]: every routing rect
//!    becomes a centerline [`Segment`] (length = the rect's longer side, so RC
//!    wirelength is the wire's run, not its width); each via rect bumps `vias`.
//! 5. **Name** nets from `TEXT` labels that land inside a net's geometry; unlabelled
//!    nets get a stable `gnet_<n>` id. Labels do not affect RC, only readability.
//!
//! ## Honest bounds (the part that keeps this MEDIUM, not a field solver)
//!
//! - **Axis-aligned rectangles only.** Each routing shape is taken as a rectangle
//!   (`Rect::from_boundary`, else its bounding box). An L-bend drawn as one polygon
//!   is bbox'd — its diagonal span over-states length. Routes drawn as several
//!   abutting rectangles (the common case) trace correctly. `PATH` records are
//!   read as their bbox; centerline-from-path-width is not modelled.
//! - **Via = overlap, no enclosure DRC** (same simplification as `vyges-lvs`'s
//!   contact gating). One via rect = one via of resistance.
//! - **Pins** are the labelled net name only; instance/pin association (`DefNet.pins`)
//!   is left empty (GDS carries no instance hookup) — so the SPEF `*CONN`/RC-tree
//!   uses the lumped form. A DEF keeps pin hookup; this is the documented gap.
//!
//! Pure std + the shared GDS kernel — unit-tested offline. See the WI-5 design
//! note in the README "Domain coverage" section for the path-vs-DEF trade-offs.

use std::collections::BTreeMap;

use vyges_layout::flatten::flatten;
use vyges_layout::gds::{Cell, Element, Library};
use vyges_layout::geom::{bbox, Rect};

use crate::def::{DefNet, Segment};

/// GDS layer + datatype key.
pub type Ld = (i16, i16);

/// Layer map: `GDS layer/datatype → role`. A tiny `key: value` deck, mirroring
/// the `vyges-lvs` extract rules shape.
///
/// ```text
/// # role     gds_layer/datatype   routing-name
/// routing:   68/20                met1
/// routing:   69/20                met2
/// routing:   70/20                met3
/// via:       67/44                          # mcon / via cut
/// via:       68/44
/// label:     68/5                           # TEXT layer carrying net names
/// ```
#[derive(Debug, Clone, Default)]
pub struct LayerMap {
    routing: BTreeMap<Ld, String>,
    vias: Vec<Ld>,
    labels: Vec<Ld>,
}

#[derive(Debug)]
pub struct GdsError(pub String);
impl std::fmt::Display for GdsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "gds error: {}", self.0)
    }
}
impl std::error::Error for GdsError {}

fn parse_ld(s: &str) -> Result<Ld, GdsError> {
    let (a, b) = s.trim().split_once('/').unwrap_or((s.trim(), "0"));
    Ok((
        a.trim().parse().map_err(|_| GdsError(format!("bad layer/datatype {s:?}")))?,
        b.trim().parse().map_err(|_| GdsError(format!("bad layer/datatype {s:?}")))?,
    ))
}

impl LayerMap {
    pub fn parse(text: &str) -> Result<LayerMap, GdsError> {
        let mut m = LayerMap::default();
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line
                .split_once(':')
                .ok_or_else(|| GdsError(format!("expected 'key: value', got {line:?}")))?;
            let toks: Vec<&str> = v.split_whitespace().collect();
            match k.trim().to_lowercase().as_str() {
                "routing" if toks.len() == 2 => {
                    m.routing.insert(parse_ld(toks[0])?, toks[1].to_string());
                }
                "via" if !toks.is_empty() => m.vias.push(parse_ld(toks[0])?),
                "label" if !toks.is_empty() => m.labels.push(parse_ld(toks[0])?),
                other => return Err(GdsError(format!("unknown/!malformed rule: {other:?} {v:?}"))),
            }
        }
        if m.routing.is_empty() {
            return Err(GdsError("layer map has no `routing:` layers".into()));
        }
        Ok(m)
    }

    pub fn load(path: &str) -> Result<LayerMap, GdsError> {
        let text =
            std::fs::read_to_string(path).map_err(|e| GdsError(format!("{path}: {e}")))?;
        LayerMap::parse(&text)
    }

    fn is_via(&self, ld: Ld) -> bool {
        self.vias.contains(&ld)
    }
    fn is_label(&self, ld: Ld) -> bool {
        self.labels.contains(&ld)
    }
}

// ── union-find ──────────────────────────────────────────────────────────────

struct Uf {
    p: Vec<usize>,
}
impl Uf {
    fn new(n: usize) -> Uf {
        Uf { p: (0..n).collect() }
    }
    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.p[r] != r {
            r = self.p[r];
        }
        let mut c = x;
        while self.p[c] != r {
            let n = self.p[c];
            self.p[c] = r;
            c = n;
        }
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.p[a] = b;
        }
    }
}

/// Axis-aligned overlap (touching counts — abutting wires are connected).
fn overlap(a: &Rect, b: &Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

/// A classified shape: a routing rect (with its layer name) or a via rect.
struct Shape {
    rect: Rect,
    layer: Option<String>, // Some(name) = routing; None = via
}

/// Pull the axis-aligned rectangle for a Boundary/Box/Path element on layer `ld`.
fn shape_rect(el: &Element) -> Option<(Ld, Rect)> {
    match el {
        Element::Boundary { layer, datatype, pts } | Element::Box { layer, boxtype: datatype, pts } => {
            let r = Rect::from_boundary(pts).or_else(|| bbox(pts))?;
            Some(((*layer, *datatype), r))
        }
        // PATH read as its bbox (centerline-from-width is not modelled — see bounds).
        Element::Path { layer, datatype, pts, .. } => Some(((*layer, *datatype), bbox(pts)?)),
        _ => None,
    }
}

/// Convert a routing rectangle to a centerline `Segment`: the run is along the
/// rect's longer side; the short side is the wire width (not RC length).
fn rect_to_segment(layer: &str, r: &Rect, scale: f64) -> Segment {
    let w = (r.x1 - r.x0) as f64;
    let h = (r.y1 - r.y0) as f64;
    let cx = (r.x0 + r.x1) as f64 / 2.0 / scale;
    let cy = (r.y0 + r.y1) as f64 / 2.0 / scale;
    if w >= h {
        // horizontal centerline at cy spanning x0..x1
        Segment { layer: layer.into(), x0: r.x0 as f64 / scale, y0: cy, x1: r.x1 as f64 / scale, y1: cy }
    } else {
        // vertical centerline at cx spanning y0..y1
        Segment { layer: layer.into(), x0: cx, y0: r.y0 as f64 / scale, x1: cx, y1: r.y1 as f64 / scale }
    }
}

/// Trace a flattened cell's connected wire geometry into `DefNet`s, in microns.
fn trace_cell(cell: &Cell, map: &LayerMap, dbu_per_um: f64) -> Vec<DefNet> {
    // 1. classify shapes; collect labels separately.
    let mut shapes: Vec<Shape> = Vec::new();
    let mut labels: Vec<(String, i32, i32)> = Vec::new();
    for el in &cell.elements {
        if let Element::Text { layer, texttype, x, y, string } = el {
            if map.is_label((*layer, *texttype)) {
                labels.push((string.clone(), *x, *y));
            }
            continue;
        }
        if let Some((ld, rect)) = shape_rect(el) {
            if let Some(name) = map.routing.get(&ld) {
                shapes.push(Shape { rect, layer: Some(name.clone()) });
            } else if map.is_via(ld) {
                shapes.push(Shape { rect, layer: None });
            }
        }
    }

    // 2. union-find over shapes.
    let mut uf = Uf::new(shapes.len());
    for i in 0..shapes.len() {
        for j in (i + 1)..shapes.len() {
            if !overlap(&shapes[i].rect, &shapes[j].rect) {
                continue;
            }
            match (&shapes[i].layer, &shapes[j].layer) {
                // same-layer routing touch -> connected wire
                (Some(a), Some(b)) if a == b => uf.union(i, j),
                // different-layer routing: NOT joined directly (needs a via, below)
                (Some(_), Some(_)) => {}
                // a via overlapping any shape pulls them together (contact-gated)
                _ => uf.union(i, j),
            }
        }
    }

    // 3. group shapes by component root.
    let mut comps: BTreeMap<usize, (Vec<usize>, usize)> = BTreeMap::new(); // root -> (routing idx, via count)
    for i in 0..shapes.len() {
        let r = uf.find(i);
        let e = comps.entry(r).or_default();
        if shapes[i].layer.is_some() {
            e.0.push(i);
        } else {
            e.1 += 1;
        }
    }

    // 4. emit one DefNet per component that has routing geometry.
    let mut nets: Vec<DefNet> = Vec::new();
    let mut anon = 0usize;
    for (_root, (routing_idx, via_count)) in comps {
        if routing_idx.is_empty() {
            continue; // a via with no routing (shouldn't happen) — skip
        }
        let segments: Vec<Segment> = routing_idx
            .iter()
            .map(|&i| rect_to_segment(shapes[i].layer.as_ref().unwrap(), &shapes[i].rect, dbu_per_um))
            .collect();
        // name from a label landing on any of this net's rects
        let name = labels
            .iter()
            .find(|(_, lx, ly)| routing_idx.iter().any(|&i| {
                let r = &shapes[i].rect;
                *lx >= r.x0 && *lx <= r.x1 && *ly >= r.y0 && *ly <= r.y1
            }))
            .map(|(n, _, _)| n.clone())
            .unwrap_or_else(|| {
                anon += 1;
                format!("gnet_{anon}")
            });
        nets.push(DefNet { name, pins: Vec::new(), segments, vias: via_count });
    }
    nets.sort_by(|a, b| a.name.cmp(&b.name));
    nets
}

/// Trace connectivity from a GDS `Library` (top cell flattened) into `DefNet`s.
pub fn trace_library(lib: &Library, top: &str, map: &LayerMap) -> Result<Vec<DefNet>, GdsError> {
    let flat = flatten(lib, top).map_err(GdsError)?;
    // GDS db_unit is metres/dbu; dbu per micron = 1e-6 / db_unit.
    let dbu_per_um = if lib.db_unit > 0.0 { 1e-6 / lib.db_unit } else { 1000.0 };
    Ok(trace_cell(&flat, map, dbu_per_um))
}

/// Load a GDS file + layer map and trace `DefNet`s (the `gds -> DefNet` on-ramp).
pub fn trace_gds(gds_path: &str, top: &str, map_path: &str) -> Result<Vec<DefNet>, GdsError> {
    let lib = Library::load(gds_path).map_err(|e| GdsError(e.to_string()))?;
    let map = LayerMap::load(map_path)?;
    trace_library(&lib, top, &map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vyges_layout::gds::{Cell, Element, Library};

    fn rect(layer: i16, datatype: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> Element {
        Element::Boundary {
            layer,
            datatype,
            pts: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)],
        }
    }

    fn map() -> LayerMap {
        // met1 = 68/20, met2 = 69/20, via = 67/44, label = 68/5
        LayerMap::parse("routing: 68/20 met1\nrouting: 69/20 met2\nvia: 67/44\nlabel: 68/5\n").unwrap()
    }

    #[test]
    fn touching_same_layer_rects_are_one_net() {
        // two abutting met1 rects (1000 dbu/um) -> one net, two segments
        let cell = Cell {
            name: "top".into(),
            elements: vec![
                rect(68, 20, 0, 0, 2000, 200),     // 2um run
                rect(68, 20, 2000, 0, 5000, 200),  // abuts -> same net
            ],
        };
        let nets = trace_cell(&cell, &map(), 1000.0);
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].segments.len(), 2);
        // total met1 run = 2um + 3um = 5um
        let total: f64 = nets[0].segments.iter().map(|s| s.len_um()).sum();
        assert!((total - 5.0).abs() < 1e-9, "len={total}");
    }

    #[test]
    fn different_layers_join_only_through_a_via() {
        // met1 and met2 rects that overlap but NO via -> two separate nets
        let no_via = Cell {
            name: "t".into(),
            elements: vec![rect(68, 20, 0, 0, 3000, 200), rect(69, 20, 2800, 0, 6000, 200)],
        };
        assert_eq!(trace_cell(&no_via, &map(), 1000.0).len(), 2, "no via -> not shorted");

        // add a via cut overlapping both -> one net, via_count 1
        let with_via = Cell {
            name: "t".into(),
            elements: vec![
                rect(68, 20, 0, 0, 3000, 200),
                rect(69, 20, 2800, 0, 6000, 200),
                rect(67, 44, 2850, 50, 2950, 150), // via overlaps both metals
            ],
        };
        let nets = trace_cell(&with_via, &map(), 1000.0);
        assert_eq!(nets.len(), 1, "via joins the layers");
        assert_eq!(nets[0].vias, 1, "the via cut is counted");
        assert_eq!(nets[0].segments.len(), 2);
    }

    #[test]
    fn text_labels_name_nets() {
        let cell = Cell {
            name: "t".into(),
            elements: vec![
                rect(68, 20, 0, 0, 4000, 200),
                Element::Text { layer: 68, texttype: 5, x: 1000, y: 100, string: "vbias".into() },
            ],
        };
        let nets = trace_cell(&cell, &map(), 1000.0);
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].name, "vbias");
    }

    #[test]
    fn unlabelled_nets_get_stable_ids() {
        let cell = Cell {
            name: "t".into(),
            elements: vec![rect(68, 20, 0, 0, 4000, 200)],
        };
        let nets = trace_cell(&cell, &map(), 1000.0);
        assert_eq!(nets[0].name, "gnet_1");
    }

    #[test]
    fn segment_run_is_the_long_side_not_the_width() {
        // a 5um-long, 0.2um-wide met1 rect -> 5um centerline, not 5.2 or width.
        let cell = Cell { name: "t".into(), elements: vec![rect(68, 20, 0, 0, 5000, 200)] };
        let nets = trace_cell(&cell, &map(), 1000.0);
        let s = &nets[0].segments[0];
        assert!((s.len_um() - 5.0).abs() < 1e-9, "len={}", s.len_um());
        assert!(s.is_horizontal());
    }

    #[test]
    fn library_round_trip_through_flatten() {
        // a real GDS Library -> trace_library (exercises flatten + db_unit scale).
        let mut lib = Library::default(); // db_unit 1e-9 -> 1000 dbu/um
        lib.name = "TOP".into();
        lib.cells.push(Cell {
            name: "top".into(),
            elements: vec![
                rect(68, 20, 0, 0, 10000, 140), // 10um met1
                Element::Text { layer: 68, texttype: 5, x: 5000, y: 70, string: "bias".into() },
            ],
        });
        let nets = trace_library(&lib, "top", &map()).unwrap();
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].name, "bias");
        assert!((nets[0].segments[0].len_um() - 10.0).abs() < 1e-9);
    }
}
