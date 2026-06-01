//! Analytic 2.5-D capacitance kernel — the geometry-derived coupling model (M5).
//!
//! The rule-based path uses a hand-tuned per-layer `coupling_per_um` coefficient
//! that scales as `s_ref/gap`. The OpenRCX correlation showed that coefficient is
//! the dominant error, and that it had the layer trend *backwards* (the rules made
//! thicker upper metals couple less, when physically a taller sidewall couples
//! more). This kernel replaces the coefficient with the **sidewall parallel-plate**
//! capacitance from the actual metal thickness and edge-to-edge gap:
//!
//! ```text
//!   Cc(per um of parallel run) = eps_r · eps0 · T / max(gap, gap_min)
//! ```
//!
//! `T` (metal thickness) comes from the tech LEF, `gap` is the real edge-to-edge
//! spacing from the layout, and `eps_r` (effective permittivity) is the single knob
//! calibrated against the golden — replacing six per-layer coefficients with one
//! physical parameter and the correct geometry dependence. A pure parallel-plate
//! over-states coupling at wide spacing, so `gap` is floored at `gap_min` (the
//! near-field plateau), which also keeps abutting wires finite.
//!
//! Pure std — unit-tested offline. The full field-solved / pattern-fit kernel
//! (fringe-coupling, multi-neighbour shielding) is the next M5 step.

/// Vacuum permittivity in fF per micron: 8.854e-12 F/m = 8.854e-3 fF/um.
pub const EPS0_FF_PER_UM: f64 = 8.854e-3;

/// Minimum effective gap (um) — the near-field plateau below which the
/// parallel-plate `1/gap` is clamped. Set near the design min metal spacing: below
/// it the bare parallel-plate diverges and over-states coupling (the rule path
/// capped the same way at `s_ref`), and sub-spacing gaps are usually same-net.
pub const GAP_MIN_UM: f64 = 0.2;

/// Lateral coupling capacitance per micron of parallel run (fF/um) between two
/// same-layer wires at edge-to-edge `gap_um`, for metal of `thickness_um`.
pub fn coupling_per_um(eps_r: f64, thickness_um: f64, gap_um: f64) -> f64 {
    eps_r * EPS0_FF_PER_UM * thickness_um / gap_um.max(GAP_MIN_UM)
}
