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
    // per-pin RC tree (driver clkbuf + 1 sink ff0): C split to pin nodes,
    // trunk + branch each R/2 from the net-node root
    assert!(s.contains("*CAP\n1 *2:X 0.225000")); // near half at the driver
    assert!(s.contains("2 *3:CLK 0.225000")); // far half at the sink
    assert!(s.contains("*RES\n1 *1 *2:X 5.025000")); // trunk net->driver
    assert!(s.contains("2 *1 *3:CLK 5.025000")); // branch net->sink
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
    let cpl = vec![CouplingCap {
        a: "clk".into(),
        b: "n0".into(),
        cap_ff: 0.03,
    }];
    let s = render("c", &Units::default(), None, &[net(), n0], &cpl);
    // totals include the coupling on BOTH nets (clk id=1, n0 id=2)
    assert!(s.contains("*D_NET *1 0.480000"), "clk total"); // 0.45 + 0.03
    assert!(s.contains("*D_NET *2 0.230000"), "n0 total"); // 0.20 + 0.03
                                                           // coupling listed once, under net A (clk), as a two-node cap entry
    assert!(s.contains(" *1 *2 0.030000"), "coupling cap entry\n{s}");
}

#[test]
fn deterministic_repeatable() {
    let a = render("counter", &Units::default(), None, &[net()], &[]);
    let b = render("counter", &Units::default(), None, &[net()], &[]);
    assert_eq!(a, b);
}

#[test]
fn native_conn_hookup_direction_and_cin() {
    use vyges_extract::def::Def;
    use vyges_extract::hookup::PinResolver;
    use vyges_extract::spef::render_distributed;
    use vyges_loom::lef::Lef;
    use vyges_loom::liberty::Lib;

    // clk driven by clkbuf:X (output), loaded by ff0:CLK (input, Cin 3 fF)
    let def = Def::parse(
        "VERSION 5.8 ;\nDESIGN counter ;\nUNITS DISTANCE MICRONS 1000 ;\n\
         COMPONENTS 2 ;\n- clkbuf CLKBUF_X1 + PLACED ( 0 0 ) N ;\n- ff0 DFF_X1 + PLACED ( 1 0 ) N ;\nEND COMPONENTS\n\
         NETS 1 ;\n- clk ( clkbuf X ) ( ff0 CLK ) ;\nEND NETS\nEND DESIGN\n",
    )
    .unwrap();
    let lef = Lef::parse(
        "MACRO CLKBUF_X1\n PIN X\n  DIRECTION OUTPUT ;\n END X\nEND CLKBUF_X1\n\
         MACRO DFF_X1\n PIN CLK\n  DIRECTION INPUT ;\n END CLK\nEND DFF_X1\n",
    )
    .unwrap();
    let lib = Lib::parse(
        "library(d){ capacitive_load_unit (1, ff);\n cell(CLKBUF_X1){pin(X){direction:output;}}\n cell(DFF_X1){pin(CLK){direction:input; capacitance:3.0;}} }\n",
    )
    .unwrap();
    let resolver = PinResolver::from_loaded(&def, Some(lef), Some(lib));

    let none: Vec<Option<vyges_extract::tree::RcNetwork>> = vec![None];
    let s = render_distributed(
        "counter",
        &Units::default(),
        None,
        &[net()],
        &none,
        &[],
        Some(&resolver),
    );
    // driver marked O, load marked I with its Cin
    assert!(s.contains(":X O\n"), "spef:\n{s}");
    assert!(s.contains(":CLK I *L 3"), "spef:\n{s}");
    // net cap grew by the 3 fF load Cin (0.45 wire + 3.0)
    assert!(s.contains("*D_NET *1 3.45"), "spef:\n{s}");
}
