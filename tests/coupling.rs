use std::collections::BTreeMap;

use vyges_extract::coupling::extract_coupling;
use vyges_extract::def::{DefNet, Segment};
use vyges_extract::rules::RcRules;

fn no_widths() -> BTreeMap<String, f64> {
    BTreeMap::new()
}

fn hnet(name: &str, x0: f64, x1: f64, y: f64) -> DefNet {
    DefNet {
        name: name.into(),
        pins: vec![],
        segments: vec![Segment { layer: "met1".into(), x0, y0: y, x1, y1: y }],
        vias: 0,
    }
}

#[test]
fn parallel_segments_couple() {
    // met1: coupling 0.1 fF/um at s_ref 0.5um (cols: res cap coupling s_ref)
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\n").unwrap();
    // a: x[0,3] @ y0 ; b: x[1,5] @ y0.5  -> overlap_x = 2.0, gap = 0.5 (== s_ref -> factor 1.0)
    let cc = extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), hnet("b", 1.0, 5.0, 0.5)], &rules, &no_widths());
    assert_eq!(cc.len(), 1);
    assert_eq!((cc[0].a.as_str(), cc[0].b.as_str()), ("a", "b"));
    // Cc = 0.1 * 2.0 * (0.5 / max(0.5,0.5)) = 0.2
    assert!((cc[0].cap_ff - 0.2).abs() < 1e-9, "cc={}", cc[0].cap_ff);
}

#[test]
fn wider_gap_couples_less() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\n").unwrap();
    let near = extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 0.5)], &rules, &no_widths());
    let far = extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 1.5)], &rules, &no_widths());
    assert!(far[0].cap_ff < near[0].cap_ff, "1/gap falloff");
}

#[test]
fn beyond_cutoff_no_coupling() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 1.0\n").unwrap();
    // gap 1.5 > cutoff 1.0
    let cc = extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 1.5)], &rules, &no_widths());
    assert!(cc.is_empty());
}

#[test]
fn lef_width_uses_edge_gap() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\n").unwrap();
    let mut widths = BTreeMap::new();
    widths.insert("met1".to_string(), 0.4); // wide wires -> smaller edge gap
    // centerline gap 1.0 ; edge gap = 1.0 - 0.4 = 0.6 ; overlap 3.0 ; s_ref 0.5
    let with_w = extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 1.0)], &rules, &widths);
    let plain = extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 1.0)], &rules, &no_widths());
    // edge gap (0.6) < centerline (1.0) -> stronger coupling
    assert!(with_w[0].cap_ff > plain[0].cap_ff);
    // Cc = 0.1 * 3.0 * (0.5 / max(0.6,0.5)) = 0.3 * (0.5/0.6) = 0.25
    assert!((with_w[0].cap_ff - 0.25).abs() < 1e-9, "cc={}", with_w[0].cap_ff);
}

#[test]
fn different_layers_dont_couple() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\nmet2 0.1 0.05 0.1 0.5\n").unwrap();
    let mut b = hnet("b", 0.0, 3.0, 0.5);
    b.segments[0].layer = "met2".into();
    assert!(extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), b], &rules, &no_widths()).is_empty());
}
