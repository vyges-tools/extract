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
            Segment { layer: "met1".into(), x0: 0.0, y0: 0.0, x1: 3.0, y1: 0.0 },
            Segment { layer: "met2".into(), x0: 0.0, y0: 0.0, x1: 3.0, y1: 0.0 },
        ],
        vias: 1,
    };
    let p = extract_net(&net, &rules()).unwrap();
    // R = 3*0.125 + 3*0.125 + 1*9.3 = 10.05 ; C = 3*0.078 + 3*0.072 = 0.45
    assert!((p.res_ohm - 10.05).abs() < 1e-9, "res={}", p.res_ohm);
    assert!((p.cap_ff - 0.45).abs() < 1e-9, "cap={}", p.cap_ff);
}

#[test]
fn unknown_layer_is_an_error_not_silent() {
    let net = DefNet {
        name: "x".into(),
        pins: vec![],
        segments: vec![Segment { layer: "met9".into(), x0: 0.0, y0: 0.0, x1: 1.0, y1: 0.0 }],
        vias: 0,
    };
    assert!(extract_net(&net, &rules()).is_err());
}
