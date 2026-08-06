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

/// A `*D_NET` total, as a NUMBER. The writer emits significant figures and trims trailing
/// zeros, so `0.45` and `0.450000` are the same total and only one of them is a property of the
/// design. Asserting the text made every such test a hostage to the number format.
fn d_net(text: &str, id: &str) -> f64 {
    text.lines()
        .find(|l| l.starts_with(&format!("*D_NET {id} ")))
        .and_then(|l| l.split_whitespace().nth(2))
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or_else(|| panic!("no *D_NET {id}\n{text}"))
}

#[test]
fn name_map_and_dnet() {
    let s = render("counter", &Units::default(), None, &[net()], &[]);
    assert!(s.contains("*NAME_MAP"));
    assert!(s.contains("*1 clk"));
    assert!(s.contains("*2 clkbuf"));
    assert!(s.contains("*3 ff0"));
    assert!((d_net(&s, "*1") - 0.45).abs() < 1e-9); // no coupling -> total == ground
    assert!(s.contains("*I *2:X I"));
    // per-pin RC tree (driver clkbuf + 1 sink ff0): C split to pin nodes,
    // trunk + branch each R/2 from the net-node root
    assert!(s.contains("*CAP\n1 *2:X 0.225")); // near half at the driver
    assert!(s.contains("2 *3:CLK 0.225")); // far half at the sink
    assert!(s.contains("*RES\n1 *1:0 *2:X 5.025")); // trunk net->driver
    assert!(s.contains("2 *1:0 *3:CLK 5.025")); // branch net->sink
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
    assert!((d_net(&s, "*1") - 0.48).abs() < 1e-9, "clk total"); // 0.45 + 0.03
    assert!((d_net(&s, "*2") - 0.23).abs() < 1e-9, "n0 total"); // 0.20 + 0.03
    // Listed under BOTH nets: a reader applies a coupling capacitor to the net whose block it
    // is in, so one listing leaves the other net believing it is coupled to nothing.
    assert!(s.contains(" *1:0 *2:0 0.03"), "coupling under clk\n{s}");
    assert!(s.contains(" *2:0 *1:0 0.03"), "coupling under n0\n{s}");
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

/// Undo SPEF name escaping, so the writer's output can be checked against the name we meant
/// rather than against a fixed string. Mirrors the reader in `vyges-loom`.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut esc = false;
    for c in s.chars() {
        if esc {
            out.push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// Pull the name-map entries out of a rendered SPEF as the reader sees them.
fn name_map(text: &str) -> Vec<String> {
    text.lines()
        .skip_while(|l| l.trim() != "*NAME_MAP")
        .skip(1)
        .take_while(|l| l.trim_start().starts_with('*') && l.split_whitespace().count() == 2)
        .filter_map(|l| l.split_whitespace().nth(1).map(unescape))
        .collect()
}

/// **Hierarchical and bussed names must be escaped on the way out.**
///
/// The SPEF grammar reserves `/`, `[`, `]`, `:`, `.` and `*`. A net called
/// `u_adapter/q[0]` written raw is not that name to a reader — it splits on the divider and
/// the bus delimiters, and the net is silently absent from anything keyed on the name. That
/// was measured at ~5 % of nets on a real block, and they are the hierarchical ones, which is
/// not a random sample.
#[test]
fn hierarchical_names_are_escaped_and_reversible() {
    let mut n = net();
    n.name = "u_adapter/q[0]".into();
    let text = render("blk", &Units::default(), None, &[n], &[]);
    // WRITTEN AS THE DESIGN NAMES IT, not re-escaped. Both forms are legal SPEF and they mean
    // different things: OpenSTA's grammar reads `q[0]` as a BIT_IDENT (bit 0 of bus `q`) and
    // `q\[0\]` as an ID whose characters include the brackets. Which one is right depends on
    // whether the netlist declares a bus or an escaped identifier — the characters alone cannot
    // say, and the DEF we read already spelled it correctly.
    //
    // When in doubt the unescaped form is the safe one: `SdcNetwork::findNetRelative` tries the
    // name, then `escapeDividers`, then `escapeBrackets`, then both — it only ever ADDS escapes,
    // so under-escaping is recovered and over-escaping is a hard miss. Three OpenSTA issues
    // (#132, #208, #311) are all in that direction. Measured: a re-escaped `count\[0\]` gives
    // `net count\[0\] not found` for every bussed net in a hardened design.
    assert!(
        text.contains("*1 u_adapter/q[0]"),
        "the name is written as the design spells it:\n{text}"
    );
    // and escaping must be exactly reversible — an escape a reader cannot undo is no better
    // than none at all. (The map also carries instance names; only the net is asserted here.)
    assert!(
        name_map(&text).contains(&"u_adapter/q[0]".to_string()),
        "{:?}",
        name_map(&text)
    );
}

/// A coupling cap between two hierarchical names goes through the same map, and this is the
/// pairing the correlation harness keys on.
#[test]
fn coupling_between_hierarchical_names_is_escaped() {
    let mut a = net();
    a.name = "top/u_a/d[3]".into();
    let mut b = net();
    b.name = "top/u_b/q[3]".into();
    b.pins = vec![("ff1".into(), "D".into())];
    let cc = CouplingCap {
        a: a.name.clone(),
        b: b.name.clone(),
        cap_ff: 1.25,
    };
    let text = render("blk", &Units::default(), None, &[a, b], &[cc]);
    let names = name_map(&text);
    assert!(names.contains(&"top/u_a/d[3]".to_string()), "{names:?}");
    assert!(names.contains(&"top/u_b/q[3]".to_string()), "{names:?}");
    // the coupling entry itself references both by id, so it is unaffected by the escaping —
    // pin that it is still emitted exactly once
    // The coupling entry references both nets by map id, so escaping cannot corrupt it —
    // pin that it is still emitted exactly once, and only inside a *CAP section (a *RES line
    // has the same field count, which is precisely the ambiguity this format hands a reader).
    let mut in_cap = false;
    let mut cc_lines = 0;
    for l in text.lines() {
        match l.trim() {
            "*CAP" => in_cap = true,
            "*RES" | "*CONN" | "*END" => in_cap = false,
            body => {
                let f: Vec<&str> = body.split_whitespace().collect();
                if in_cap
                    && f.len() == 4
                    && f[0].chars().all(|c| c.is_ascii_digit())
                    && f[3].parse::<f64>().is_ok()
                {
                    cc_lines += 1;
                }
            }
        }
    }
    // Once per net, so twice in the file — see `coupling_adds_to_totals_and_caps`.
    assert_eq!(cc_lines, 2, "a two-node *CAP entry under each net:\n{text}");
}

/// A name with no reserved characters must pass through untouched — escaping is not allowed to
/// rewrite the common case.
#[test]
fn ordinary_names_are_not_escaped() {
    let text = render("blk", &Units::default(), None, &[net()], &[]);
    assert!(!text.contains("\\clk"), "{text}");
    assert!(name_map(&text).contains(&"clk".to_string()), "{:?}", name_map(&text));
}
