use vyges_extract::rc::NetParasitics;
use vyges_extract::spef::{render, Units};

fn net() -> NetParasitics {
    NetParasitics {
        name: "clk".into(),
        pins: vec![("clkbuf".into(), "X".into()), ("ff0".into(), "CLK".into())],
        res_ohm: 10.05,
        cap_ff: 0.45,
    }
}

#[test]
fn header_and_units() {
    let s = render("counter", &Units::default(), None, &[net()]);
    assert!(s.starts_with("*SPEF \"IEEE 1481-1999\""));
    assert!(s.contains("*DESIGN \"counter\""));
    assert!(s.contains("*PROGRAM \"vyges-extract\""));
    assert!(s.contains("*C_UNIT 1 FF"));
    assert!(s.contains("*R_UNIT 1 OHM"));
    assert!(!s.contains("*DATE")); // omitted -> reproducible
}

#[test]
fn name_map_and_dnet() {
    let s = render("counter", &Units::default(), None, &[net()]);
    assert!(s.contains("*NAME_MAP"));
    assert!(s.contains("*1 clk"));
    assert!(s.contains("*2 clkbuf"));
    assert!(s.contains("*3 ff0"));
    assert!(s.contains("*D_NET *1 0.450000"));
    assert!(s.contains("*I *2:X I"));
    assert!(s.contains("*CAP\n1 *1 0.450000"));
    assert!(s.contains("*RES\n1 *1 *2:X 10.050000"));
    assert!(s.trim_end().ends_with("*END"));
}

#[test]
fn deterministic_repeatable() {
    let a = render("counter", &Units::default(), None, &[net()]);
    let b = render("counter", &Units::default(), None, &[net()]);
    assert_eq!(a, b);
}
