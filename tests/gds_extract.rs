//! `gds → DefNet → RC` end to end: the optional GDS-only on-ramp feeds the
//! *unchanged* RC core. Proves the `DefNet` boundary holds — geometry traced from
//! raw GDS extracts exactly as geometry read from a routed DEF would.

use vyges_extract::coupling::extract_coupling;
use vyges_extract::gds::{trace_library, LayerMap};
use vyges_extract::rc::extract_net;
use vyges_extract::rules::RcRules;
use vyges_layout::gds::{Cell, Element, Library};

fn rect(layer: i16, datatype: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> Element {
    Element::Boundary { layer, datatype, pts: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)] }
}
fn label(s: &str, x: i32, y: i32) -> Element {
    Element::Text { layer: 68, texttype: 5, x, y, string: s.into() }
}

fn layer_map() -> LayerMap {
    LayerMap::parse("routing: 68/20 met1\nrouting: 69/20 met2\nvia: 67/44\nlabel: 68/5\n").unwrap()
}

#[test]
fn gds_traced_nets_extract_through_the_rc_core() {
    // A tiny analog-style GDS: a 20um bias line on met1, and a sensitive node that
    // goes met1 (10um) -> via -> met2 (10um), running parallel to the bias line.
    // 1000 dbu/um (default db_unit 1e-9). All values hand-checkable.
    let mut lib = Library::default();
    lib.name = "TOP".into();
    lib.cells.push(Cell {
        name: "top".into(),
        elements: vec![
            // vbias: met1 vertical, x~=1um, y 1..21um (20um), 0.14um wide
            rect(68, 20, 930, 1000, 1070, 21000),
            label("vbias", 1000, 11000),
            // vsense: met1 vertical x~=2um y 1..11um (10um) ...
            rect(68, 20, 1930, 1000, 2070, 11000),
            // ... via up to met2 ...
            rect(67, 44, 1950, 10900, 2050, 11000),
            // ... met2 horizontal y~=11um x 2..12um (10um)
            rect(69, 20, 2000, 10930, 12000, 11070),
            label("vsense", 2000, 6000),
        ],
    });

    let nets = trace_library(&lib, "top", &layer_map()).expect("trace");
    assert_eq!(nets.len(), 2, "vbias + vsense");

    // Feed the UNCHANGED RC core (rc.rs) — same rules deck the DEF path uses.
    let rules = RcRules::parse(
        "met1 0.125 0.078 0.050 0.14\nmet2 0.125 0.072 0.044 0.14\nvia 9.3\ncouple_cutoff 2.0\n",
    )
    .unwrap();

    let vbias = nets.iter().find(|n| n.name == "vbias").unwrap();
    let vsense = nets.iter().find(|n| n.name == "vsense").unwrap();

    let pb = extract_net(vbias, &rules, &std::collections::BTreeMap::new()).unwrap();
    let ps = extract_net(vsense, &rules, &std::collections::BTreeMap::new()).unwrap();

    // vbias: 20um met1 -> R 2.5, C 1.56
    assert!((pb.res_ohm - 2.5).abs() < 1e-6, "vbias R={}", pb.res_ohm);
    assert!((pb.cap_ff - 1.56).abs() < 1e-6, "vbias C={}", pb.cap_ff);
    // vsense: 10um met1 + 10um met2 + 1 via -> R 11.8, C 1.5
    assert_eq!(vsense.vias, 1, "via traced");
    assert!((ps.res_ohm - 11.8).abs() < 1e-6, "vsense R={}", ps.res_ohm);
    assert!((ps.cap_ff - 1.5).abs() < 1e-6, "vsense C={}", ps.cap_ff);

    // Coupling between the parallel met1 legs is captured (no LEF widths here, so
    // the gap is centerline ~1um) — same coupling.rs the DEF path runs.
    let cc = extract_coupling(&nets, &rules, &Default::default(), &Default::default());
    assert_eq!(cc.len(), 1, "vbias <-> vsense couple");
    assert!(cc[0].cap_ff > 0.0);
}
