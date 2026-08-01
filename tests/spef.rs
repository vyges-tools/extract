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

    // `*DATE` and `*DESIGN_FLOW` are REQUIRED by the SPEF grammar OpenSTA implements, and
    // therefore by OpenROAD and LibreLane. Omitting either makes the file a syntax error at the
    // first missing line — a SPEF that reads fine to us and is rejected outright by everyone
    // else. This test previously asserted `*DATE` was ABSENT, in the belief that omitting it
    // was what made the output reproducible; it is not, and the belief cost us a file no
    // incumbent could read.
    assert!(
        s.contains("*DATE"),
        "required by the standard, whatever we think of it"
    );
    assert!(
        s.contains("*DESIGN_FLOW"),
        "required; also states pin caps are not included"
    );

    // Reproducibility is preserved by making the stamp FIXED, not by dropping the field.
    let again = render("counter", &Units::default(), None, &[net()], &[]);
    assert_eq!(s, again, "output must stay byte-identical across runs");
    assert!(
        !s.contains("1970-01-01T"),
        "a fixed stamp, in the format the grammar expects"
    );
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

/// Every `*<id>` in the body must resolve to an entry in `*NAME_MAP`.
///
/// This is the invariant behind four separate defects, each of which produced a *different*
/// symptom in a different reader — `*0:clk_i` (an id below the map's base), a `usize::MAX`
/// sentinel leaking into a coupling label, an instance interned that names nothing, a port
/// spelled as a reference. Asserting the specific symptoms would have caught none of the others,
/// so assert the rule: a leading `*` promises the reader a name-map lookup, and every such
/// promise here must be keepable.
///
/// Our own reader never had to check, which is exactly why our own tests never caught it —
/// only feeding the output back through OpenRCX did.
fn assert_every_reference_resolves(s: &str) {
    let mut map_ids = std::collections::BTreeSet::new();
    let mut in_map = false;
    for line in s.lines() {
        if line.starts_with("*NAME_MAP") {
            in_map = true;
            continue;
        }
        if in_map {
            if line.starts_with('*') && line[1..].starts_with(|c: char| c.is_ascii_digit()) {
                if let Some((id, _)) = line[1..].split_once(' ') {
                    map_ids.insert(id.to_string());
                }
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            in_map = false; // the map ends at the first non-entry
        }
        // Skip quoted header values, which may legitimately contain anything.
        if line.starts_with('*') && line.contains('"') {
            continue;
        }
        for tok in line.split_whitespace() {
            let Some(body) = tok.strip_prefix('*') else {
                continue;
            };
            // Section keywords (`*CAP`, `*D_NET`, …) are not references.
            let id: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
            if id.is_empty() {
                continue;
            }
            assert!(
                map_ids.contains(&id),
                "`{tok}` references name-map id {id}, which is not in the map\n  in: {line}"
            );
        }
    }
    assert!(!map_ids.is_empty(), "the scan found no name map at all");
}

#[test]
fn every_name_map_reference_resolves() {
    let s = render("counter", &Units::default(), None, &[net()], &[]);
    assert_every_reference_resolves(&s);
}

#[test]
fn a_port_is_never_written_as_a_name_map_reference() {
    // DEF spells a top-level port connection with the pseudo-instance `PIN`. It is not an
    // instance and is deliberately never interned, so any `*<id>` built from it dangles.
    let with_port = NetParasitics {
        name: "clk".into(),
        pins: vec![("PIN".into(), "clk_i".into()), ("ff0".into(), "CLK".into())],
        res_ohm: 10.0,
        cap_ff: 0.4,
    };
    let s = render("counter", &Units::default(), None, &[with_port], &[]);

    assert!(
        s.contains("*P clk_i I"),
        "the connection is a port declaration"
    );
    assert!(
        !s.contains(":clk_i"),
        "the port is its own node — `clk_i`, never `<something>:clk_i`\n{s}"
    );
    // Not a bare `contains("PIN")` — `*DESIGN_FLOW "… PIN_CAP NONE"` legitimately contains it.
    assert!(
        !s.contains("PIN:") && !s.contains(" PIN "),
        "the DEF placeholder names nothing and must not reach the file as an instance\n{s}"
    );
    assert_every_reference_resolves(&s);
}
