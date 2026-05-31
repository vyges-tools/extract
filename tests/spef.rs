use vyges_extract::coupling::CouplingCap;
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
    let s = render("counter", &Units::default(), None, &[net()], &[]);
    assert!(s.starts_with("*SPEF \"IEEE 1481-1999\""));
    assert!(s.contains("*DESIGN \"counter\""));
    assert!(s.contains("*PROGRAM \"vyges-extract\""));
    assert!(s.contains("*C_UNIT 1 FF"));
    assert!(s.contains("*R_UNIT 1 OHM"));
    assert!(!s.contains("*DATE")); // omitted -> reproducible
}

#[test]
fn name_map_and_dnet() {
    let s = render("counter", &Units::default(), None, &[net()], &[]);
    assert!(s.contains("*NAME_MAP"));
    assert!(s.contains("*1 clk"));
    assert!(s.contains("*2 clkbuf"));
    assert!(s.contains("*3 ff0"));
    assert!(s.contains("*D_NET *1 0.450000")); // no coupling -> total == ground
    assert!(s.contains("*I *2:X I"));
    assert!(s.contains("*CAP\n1 *1 0.450000"));
    assert!(s.contains("*RES\n1 *1 *2:X 10.050000"));
    assert!(s.trim_end().ends_with("*END"));
}

#[test]
fn coupling_adds_to_totals_and_caps() {
    let n0 = NetParasitics {
        name: "n0".into(),
        pins: vec![("u0".into(), "Y".into())],
        res_ohm: 1.0,
        cap_ff: 0.20,
    };
    let cpl = vec![CouplingCap { a: "clk".into(), b: "n0".into(), cap_ff: 0.03 }];
    let s = render("c", &Units::default(), None, &[net(), n0], &cpl);
    // totals include the coupling on BOTH nets (clk id=1, n0 id=2)
    assert!(s.contains("*D_NET *1 0.480000"), "clk total"); // 0.45 + 0.03
    assert!(s.contains("*D_NET *2 0.230000"), "n0 total"); // 0.20 + 0.03
    // coupling listed once, under net A (clk), as a two-node cap entry
    assert!(s.contains("2 *1 *2 0.030000"), "coupling cap entry\n{s}");
}

#[test]
fn deterministic_repeatable() {
    let a = render("counter", &Units::default(), None, &[net()], &[]);
    let b = render("counter", &Units::default(), None, &[net()], &[]);
    assert_eq!(a, b);
}
