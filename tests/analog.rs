//! Analog / mixed-signal coverage — the RC core is geometry x rules, so an
//! analog routed layout supplied as DEF extracts on exactly the same contract
//! as a digital std-cell block. No code path is std-cell- or Liberty-specific.
//!
//! `examples/bias_gen/` is a small analog bias generator with three routed nets
//! that exercise the cases analog designers care about:
//!   vbias    — a long thin bias distribution line (met1): R matters, and it runs
//!              parallel to a sensitive node so it couples to it.
//!   vsense   — a high-impedance sensitive node carried up the stack (met1->via->
//!              met2): low cap is the point, and it must capture the bias coupling.
//!   vsupply  — a wide local supply tap (met3): low R by design, larger ground cap.

use vyges_extract::engine::{extract, run_to_spef};
use vyges_extract::job::ExtractJob;

fn job() -> ExtractJob {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/bias_gen/bias_gen.ext");
    ExtractJob::load(p).expect("load bias_gen job")
}

fn net<'a>(ex: &'a vyges_extract::engine::Extraction, name: &str) -> &'a vyges_extract::rc::NetParasitics {
    ex.nets.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("net {name} missing"))
}

#[test]
fn analog_block_extracts_sane_rc() {
    let ex = extract(&job()).expect("extract analog block");
    assert_eq!(ex.nets.len(), 3, "vbias, vsense, vsupply");

    // Every net has strictly positive R and C — nothing extracted to zero.
    for n in &ex.nets {
        assert!(n.res_ohm > 0.0, "{} R>0, got {}", n.name, n.res_ohm);
        assert!(n.cap_ff > 0.0, "{} C>0, got {}", n.name, n.cap_ff);
    }

    // Hand-checkable values (geometry in um x analog.rules coefficients):
    //   vbias  : 20um met1            -> R 20*0.125 = 2.5 ; C 20*0.078 = 1.56
    //   vsense : 10um met1 + 10um met2 + 1 via
    //            -> R 10*0.125 + 10*0.125 + 9.3 = 11.8 ; C 10*0.078 + 10*0.072 = 1.5
    //   vsupply: 30um met3 + 1 via    -> R 30*0.047 + 9.3 = 10.71 ; C 30*0.068 = 2.04
    assert!((net(&ex, "vbias").res_ohm - 2.5).abs() < 1e-9);
    assert!((net(&ex, "vbias").cap_ff - 1.56).abs() < 1e-9);
    assert!((net(&ex, "vsense").res_ohm - 11.8).abs() < 1e-9);
    assert!((net(&ex, "vsense").cap_ff - 1.5).abs() < 1e-9);
    assert!((net(&ex, "vsupply").res_ohm - 10.71).abs() < 1e-9);
    assert!((net(&ex, "vsupply").cap_ff - 2.04).abs() < 1e-9);
}

#[test]
fn resistance_scales_with_wire_length() {
    let ex = extract(&job()).expect("extract");
    // The wide supply tap is 30um of met3; the bias line is 20um of met1. Both
    // pure wire (vsupply's lone via aside), and the longer run carries more total
    // wire cap. vbias is also thin met1 so per-um it is the most resistive.
    let vbias = net(&ex, "vbias");
    let vsupply = net(&ex, "vsupply");
    // met3 is ~2.7x lower ohm/um than met1, yet 30um vs 20um and a via still make
    // the supply tap the more resistive net overall — i.e. R tracks geometry+layer.
    assert!(vsupply.res_ohm > vbias.res_ohm, "supply R {} > bias R {}", vsupply.res_ohm, vbias.res_ohm);
    // Wider/longer supply tap has the larger grounded cap.
    assert!(vsupply.cap_ff > vbias.cap_ff, "supply C {} > bias C {}", vsupply.cap_ff, vbias.cap_ff);

    // Direct length scaling on a single layer: vbias is exactly twice vsense's
    // met1 run (20um vs 10um), and met1's ohm/um contribution scales 1:1.
    let met1_per_um = 0.125;
    assert!((vbias.res_ohm - 20.0 * met1_per_um).abs() < 1e-9);
}

#[test]
fn vias_add_resistance() {
    let ex = extract(&job()).expect("extract");
    // vsense routes met1 -> via -> met2 (1 via, 9.3 ohm). Strip the via and the
    // resistance is just the two wire runs; the via must add exactly its rule R.
    let vsense = net(&ex, "vsense");
    let wire_only = 10.0 * 0.125 + 10.0 * 0.125; // met1 + met2 runs, no via
    assert!(vsense.res_ohm > wire_only, "via must add R");
    assert!((vsense.res_ohm - wire_only - 9.3).abs() < 1e-9, "via contributes exactly 9.3 ohm");
}

#[test]
fn adjacent_nets_couple_sensitive_node_to_bias() {
    let ex = extract(&job()).expect("extract");
    // The sensitive node (vsense) runs its met1 leg parallel to the bias line
    // (vbias) 1um away (edge gap 0.86um with the LEF width) — coupling between them
    // is captured as a single net-pair entry.
    assert_eq!(ex.couplings.len(), 1, "one coupled pair (vbias <-> vsense)");
    let c = &ex.couplings[0];
    let pair = (c.a.as_str(), c.b.as_str());
    assert!(pair == ("vbias", "vsense") || pair == ("vsense", "vbias"), "got {pair:?}");
    assert!(c.cap_ff > 0.0, "coupling cap > 0");
    // Cc = 0.050 fF/um * 10um overlap * (s_ref 0.14 / edge gap 0.86) = 0.081395 fF.
    assert!((c.cap_ff - 0.081395).abs() < 1e-6, "cc={}", c.cap_ff);
    // The wide supply tap (met3, y=25um) is >2um (the couple_cutoff) from the
    // signal nets, so it is correctly left uncoupled.
    assert!(!ex.couplings.iter().any(|c| c.a == "vsupply" || c.b == "vsupply"));
}

#[test]
fn analog_block_renders_spef() {
    // End-to-end through the same `run` path digital uses: DEF -> rc -> SPEF.
    let spef = run_to_spef(&job()).expect("render SPEF");
    assert!(spef.contains("*DESIGN \"bias_gen\""));
    assert!(spef.contains("*1 vbias") && spef.contains("*2 vsense") && spef.contains("*3 vsupply"));
    // vbias total = ground 1.56 + coupling 0.081395 = 1.641395.
    assert!(spef.contains("*D_NET *1 1.641395"), "vbias total\n{spef}");
    // coupling listed once as a two-node *CAP entry (between the nets' real
    // representative nodes now that each net is a distributed tree).
    assert!(
        spef.lines().any(|l| l.contains("0.081395") && l.matches('*').count() == 2),
        "coupling vbias-vsense two-node entry\n{spef}"
    );
}
