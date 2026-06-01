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
