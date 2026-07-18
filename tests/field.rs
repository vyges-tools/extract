// M5 field kernel: sidewall parallel-plate coupling per unit length = eps_r·eps0·T/gap,
// with the near-field gap floor.
use vyges_extract::field::{coupling_per_um, EPS0_FF_PER_UM, GAP_MIN_UM};

#[test]
fn sidewall_parallel_plate() {
    // eps_r=4, T=0.35um, gap=0.2um -> 4 * 8.854e-3 * 0.35 / 0.2
    let c = coupling_per_um(4.0, 0.35, 0.2);
    let want = 4.0 * EPS0_FF_PER_UM * 0.35 / 0.2;
    assert!((c - want).abs() < 1e-12);
    assert!((c - 0.06198).abs() < 1e-4, "~0.062 fF/um, got {c}");
}

#[test]
fn thicker_metal_couples_more() {
    // physics: a taller sidewall couples more (the rule coefficients had it backwards)
    assert!(coupling_per_um(4.0, 1.2, 0.3) > coupling_per_um(4.0, 0.35, 0.3));
}

#[test]
fn gap_floored_at_near_field() {
    // abutting wires don't give infinite coupling
    let abut = coupling_per_um(4.0, 0.35, 0.01);
    let floor = coupling_per_um(4.0, 0.35, GAP_MIN_UM);
    assert!((abut - floor).abs() < 1e-12);
    assert!(abut.is_finite());
}

#[test]
fn fringe_falls_faster_than_plate_at_wide_spacing() {
    use vyges_extract::field::{coupling_per_um, coupling_per_um_fringe};
    // at wide spacing the ground-competition fall-off makes fringe < bare plate
    let plate = coupling_per_um(4.0, 0.35, 2.0);
    let fringe = coupling_per_um_fringe(4.0, 0.35, 1.0, 2.0);
    assert!(
        fringe < plate,
        "fringe-corrected falls below plate at S>>H: {fringe} vs {plate}"
    );
    // taller layer (more H) competes less with ground -> couples more at the same S
    let lowH = coupling_per_um_fringe(4.0, 0.35, 0.5, 1.0);
    let hiH = coupling_per_um_fringe(4.0, 0.35, 3.0, 1.0);
    assert!(
        hiH > lowH,
        "higher metal couples more (less ground shorting)"
    );
    // H<=0 falls back to the plate form
    assert!(
        (coupling_per_um_fringe(4.0, 0.35, 0.0, 1.0) - coupling_per_um(4.0, 0.35, 1.0)).abs()
            < 1e-12
    );
}

// shield_k is applied in the engine (ground cap -= k·Cc_net); a focused rules-parse
// check lives here, the end-to-end effect is the OpenRCX correlation.
#[test]
fn shield_k_parses() {
    use vyges_extract::rules::RcRules;
    let r = RcRules::parse("eps_r 2.3\nshield_k 0.5\nmet1 0.9 0.08\n").unwrap();
    assert!((r.shield_k - 0.5).abs() < 1e-12);
    assert!((r.eps_r - 2.3).abs() < 1e-12);
    // default off
    let r0 = RcRules::parse("met1 0.9 0.08\n").unwrap();
    assert_eq!(r0.shield_k, 0.0);
}
