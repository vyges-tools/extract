use vyges_extract::def::{DefNet, Segment};
use vyges_extract::rc::extract_net;
use vyges_extract::rules::RcRules;

fn rules() -> RcRules {
    RcRules::parse("met1 0.125 0.078\nmet2 0.125 0.072\nvia 9.3\n").unwrap()
}

#[test]
fn lumps_res_and_cap_over_segments_and_vias() {
    let net = DefNet {
        name: "clk".into(),
        pins: vec![("ff0".into(), "CLK".into())],
        segments: vec![
            Segment::wire("met1", 0.0, 0.0, 3.0, 0.0),
            Segment::wire("met2", 0.0, 0.0, 3.0, 0.0),
        ],
        vias: 1,
    };
    let p = extract_net(&net, &rules(), &std::collections::BTreeMap::new()).unwrap();
    // R = 3*0.125 + 3*0.125 + 1*9.3 = 10.05 ; C = 3*0.078 + 3*0.072 = 0.45
    assert!((p.res_ohm - 10.05).abs() < 1e-9, "res={}", p.res_ohm);
    assert!((p.cap_ff - 0.45).abs() < 1e-9, "cap={}", p.cap_ff);
}

#[test]
fn unknown_layer_is_an_error_not_silent() {
    let net = DefNet {
        name: "x".into(),
        pins: vec![],
        segments: vec![Segment::wire("met9", 0.0, 0.0, 1.0, 0.0)],
        vias: 0,
    };
    assert!(extract_net(&net, &rules(), &std::collections::BTreeMap::new()).is_err());
}

#[test]
fn resistance_is_width_dependent_with_sheet_rho() {
    use std::collections::BTreeMap;
    // sheet-resistance rule: R = rsheet * len / width. met1 = 0.1 ohm/square.
    let rules = RcRules::parse("met1 0.125 0.078\nrsheet met1 0.1\n").unwrap();
    let net = DefNet {
        name: "n".into(),
        pins: vec![],
        segments: vec![Segment::wire("met1", 0.0, 0.0, 10.0, 0.0)],
        vias: 0,
    };
    let mut narrow = BTreeMap::new();
    narrow.insert("met1".to_string(), 0.5); // R = 0.1 * 10 / 0.5 = 2.0
    let mut wide = BTreeMap::new();
    wide.insert("met1".to_string(), 1.0); //  R = 0.1 * 10 / 1.0 = 1.0
    let rn = extract_net(&net, &rules, &narrow).unwrap().res_ohm;
    let rw = extract_net(&net, &rules, &wide).unwrap().res_ohm;
    assert!((rn - 2.0).abs() < 1e-9, "narrow R = {rn}");
    assert!((rw - 1.0).abs() < 1e-9, "wide R = {rw}");
    assert!((rn / rw - 2.0).abs() < 1e-9, "halving width doubles R");
    // with no width supplied, falls back to the width-blind res column (0.125/um)
    let rb = extract_net(&net, &rules, &BTreeMap::new()).unwrap().res_ohm;
    assert!((rb - 1.25).abs() < 1e-9, "fallback R = {rb}");
}

#[test]
fn ndr_segment_width_overrides_layer_default() {
    use std::collections::BTreeMap;
    let rules = RcRules::parse("met1 0.125 0.078\nrsheet met1 0.1\n").unwrap();
    let mut widths = BTreeMap::new();
    widths.insert("met1".to_string(), 0.5); // LEF default width

    // default-width net: R = rsheet*len/width = 0.1*10/0.5 = 2.0
    let seg = Segment::wire("met1", 0.0, 0.0, 10.0, 0.0);
    let def_net = DefNet {
        name: "a".into(),
        pins: vec![],
        segments: vec![seg.clone()],
        vias: 0,
    };
    let r_default = extract_net(&def_net, &rules, &widths).unwrap().res_ohm;
    assert!(
        (r_default - 2.0).abs() < 1e-9,
        "default-width R = {r_default}"
    );

    // an NDR draws this wire at 1.0um -> R = 0.1*10/1.0 = 1.0, overriding the LEF 0.5
    let mut wide = seg;
    wide.width_um = 1.0;
    let ndr_net = DefNet {
        name: "b".into(),
        pins: vec![],
        segments: vec![wide],
        vias: 0,
    };
    let r_wide = extract_net(&ndr_net, &rules, &widths).unwrap().res_ohm;
    assert!(
        (r_wide - 1.0).abs() < 1e-9,
        "NDR width overrides the LEF default: {r_wide}"
    );
}
