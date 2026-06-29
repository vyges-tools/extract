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
    let cc = extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), hnet("b", 1.0, 5.0, 0.5)], &rules, &no_widths(), &no_widths());
    assert_eq!(cc.len(), 1);
    assert_eq!((cc[0].a.as_str(), cc[0].b.as_str()), ("a", "b"));
    // Cc = 0.1 * 2.0 * (0.5 / max(0.5,0.5)) = 0.2
    assert!((cc[0].cap_ff - 0.2).abs() < 1e-9, "cc={}", cc[0].cap_ff);
}

#[test]
fn wider_gap_couples_less() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\n").unwrap();
    let near = extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 0.5)], &rules, &no_widths(), &no_widths());
    let far = extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 1.5)], &rules, &no_widths(), &no_widths());
    assert!(far[0].cap_ff < near[0].cap_ff, "1/gap falloff");
}

#[test]
fn beyond_cutoff_no_coupling() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 1.0\n").unwrap();
    // gap 1.5 > cutoff 1.0
    let cc = extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 1.5)], &rules, &no_widths(), &no_widths());
    assert!(cc.is_empty());
}

#[test]
fn lef_width_uses_edge_gap() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\n").unwrap();
    let mut widths = BTreeMap::new();
    widths.insert("met1".to_string(), 0.4); // wide wires -> smaller edge gap
    // centerline gap 1.0 ; edge gap = 1.0 - 0.4 = 0.6 ; overlap 3.0 ; s_ref 0.5
    let with_w = extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 1.0)], &rules, &widths, &no_widths());
    let plain = extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 1.0)], &rules, &no_widths(), &no_widths());
    // edge gap (0.6) < centerline (1.0) -> stronger coupling
    assert!(with_w[0].cap_ff > plain[0].cap_ff);
    // Cc = 0.1 * 3.0 * (0.5 / max(0.6,0.5)) = 0.3 * (0.5/0.6) = 0.25
    assert!((with_w[0].cap_ff - 0.25).abs() < 1e-9, "cc={}", with_w[0].cap_ff);
}

#[test]
fn different_layers_dont_couple_laterally() {
    // no `interlayer` rule -> different layers contribute nothing
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\nmet2 0.1 0.05 0.1 0.5\n").unwrap();
    let mut b = hnet("b", 0.0, 3.0, 0.5);
    b.segments[0].layer = "met2".into();
    assert!(extract_coupling(&[hnet("a", 0.0, 3.0, 0.0), b], &rules, &no_widths(), &no_widths()).is_empty());
}

fn seg(layer: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> DefNet {
    DefNet {
        name: layer.into(),
        pins: vec![],
        segments: vec![Segment { layer: layer.into(), x0, y0, x1, y1 }],
        vias: 0,
    }
}

#[test]
fn interlayer_crossover_couples_by_area() {
    let rules =
        RcRules::parse("met1 0.1 0.05 0.0\nmet2 0.1 0.05 0.0\ninterlayer met1 met2 0.02\n").unwrap();
    let mut widths = BTreeMap::new();
    widths.insert("met1".to_string(), 0.4);
    widths.insert("met2".to_string(), 0.5);
    // met1 horizontal x[0,4] @ y2 (footprint y[1.8,2.2]); met2 vertical x2 y[0,4] (x[1.75,2.25])
    let mut a = seg("met1", 0.0, 2.0, 4.0, 2.0);
    a.name = "a".into();
    let mut b = seg("met2", 2.0, 0.0, 2.0, 4.0);
    b.name = "b".into();
    let cc = extract_coupling(&[a, b], &rules, &widths, &no_widths());
    assert_eq!(cc.len(), 1);
    // overlap area = 0.5 (x) * 0.4 (y) = 0.2 ; Cc = 0.02 * 0.2 = 0.004
    assert!((cc[0].cap_ff - 0.004).abs() < 1e-9, "cc={}", cc[0].cap_ff);
}

#[test]
fn interlayer_needs_widths() {
    let rules =
        RcRules::parse("met1 0.1 0.05 0.0\nmet2 0.1 0.05 0.0\ninterlayer met1 met2 0.02\n").unwrap();
    let mut a = seg("met1", 0.0, 2.0, 4.0, 2.0);
    a.name = "a".into();
    let mut b = seg("met2", 2.0, 0.0, 2.0, 4.0);
    b.name = "b".into();
    // zero-width footprints -> no overlap area -> no coupling
    assert!(extract_coupling(&[a, b], &rules, &no_widths(), &no_widths()).is_empty());
}

#[test]
fn spatial_index_matches_bruteforce_on_dense_layout() {
    // A deterministic grid of horizontal wires on two layers at varied offsets;
    // the indexed result must equal an exhaustive O(n^2) reference exactly
    // (no dropped pairs, no double counting, identical caps).
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\nmet2 0.12 0.05 0.08 0.5\n").unwrap();
    let mut nets: Vec<DefNet> = Vec::new();
    for i in 0..60u32 {
        let layer = if i % 2 == 0 { "met1" } else { "met2" };
        let y = (i as f64) * 0.31; // some pairs within cutoff, some not
        let x0 = (i % 7) as f64 * 0.2;
        let mut n = hnet(&format!("n{i}"), x0, x0 + 5.0, y);
        n.segments[0].layer = layer.into();
        n.name = format!("n{i}");
        nets.push(n);
    }
    let got = extract_coupling(&nets, &rules, &no_widths(), &no_widths());

    // exhaustive reference using the same public physics path: run each net PAIR
    // through extract_coupling in isolation and collect the nonzero results.
    let mut want: std::collections::BTreeMap<(String, String), f64> = Default::default();
    for i in 0..nets.len() {
        for j in (i + 1)..nets.len() {
            let pair = extract_coupling(
                &[nets[i].clone(), nets[j].clone()],
                &rules,
                &no_widths(),
                &no_widths(),
            );
            for c in pair {
                want.insert((c.a, c.b), c.cap_ff);
            }
        }
    }
    assert_eq!(got.len(), want.len(), "pair count must match brute force");
    for c in &got {
        let w = want.get(&(c.a.clone(), c.b.clone())).expect("pair present in reference");
        assert!((c.cap_ff - w).abs() < 1e-12, "cap mismatch for {}-{}: {} vs {}", c.a, c.b, c.cap_ff, w);
    }
}

#[test]
fn scales_to_thousands_of_nets() {
    // 4000 stacked parallel wires; only adjacent ones are within the cutoff, so
    // the answer is exactly 3999 coupling pairs. The naive all-pairs sweep would
    // be ~8M net-pair tests; the grid keeps it linear and finishes instantly.
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 0.7\n").unwrap();
    let n = 4000u32;
    let nets: Vec<DefNet> = (0..n)
        .map(|i| hnet(&format!("w{i}"), 0.0, 10.0, i as f64 * 0.5))
        .collect();
    let cc = extract_coupling(&nets, &rules, &no_widths(), &no_widths());
    assert_eq!(cc.len() as u32, n - 1, "only adjacent wires (gap 0.5 < 0.7) couple");
}
