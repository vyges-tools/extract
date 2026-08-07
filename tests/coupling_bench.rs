//! Realistic coupling-scan benchmark — a synthetic dense routed block that
//! reproduces the memory/time scaling behaviour tracked in issue #11.
//!
//! Ignored by default (it is a benchmark, not a correctness gate). Run it with a
//! release build to get meaningful numbers:
//!
//! ```text
//!   cargo test --release --test coupling_bench -- --ignored --nocapture
//! ```
//!
//! Knobs (env):
//! - `VYGES_BENCH_WIRES`   parallel wires per band          (default 20000)
//! - `VYGES_BENCH_LEN`     wire length in um                (default 200.0)
//! - `VYGES_BENCH_PITCH`   centre-to-centre wire pitch (um) (default 0.4)
//! - `VYGES_BENCH_CUTOFF`  couple_cutoff (um); bigger = more neighbours/wire = more
//!   distinct pairs (default 2.0)
//! - `VYGES_MAX_COUPLING_PAIRS`  safety-valve cap (honoured by the engine)
//!
//! Each wire couples to every neighbour within `cutoff`, so the distinct-pair count
//! grows as `wires × (cutoff / pitch)` — the knob that drove the tens-of-GB /
//! OOM behaviour on real ultra-dense blocks.

use std::time::Instant;

use vyges_extract::coupling::extract_coupling;
use vyges_extract::def::{DefNet, Segment};
use vyges_extract::rules::RcRules;

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(default)
}
fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(default)
}

#[test]
#[ignore = "benchmark; run with --release --ignored --nocapture"]
fn coupling_dense_block() {
    let wires = env_usize("VYGES_BENCH_WIRES", 20_000);
    let len = env_f64("VYGES_BENCH_LEN", 200.0);
    let pitch = env_f64("VYGES_BENCH_PITCH", 0.4);
    let cutoff = env_f64("VYGES_BENCH_CUTOFF", 2.0);

    let rules =
        RcRules::parse(&format!("met1 0.1 0.05 0.1 0.5\ncouple_cutoff {cutoff}\n")).unwrap();

    // A dense band of parallel met1 wires. Each couples to every neighbour within the
    // cutoff, so the expected distinct-pair count is ~wires × neighbours-per-side.
    let t0 = Instant::now();
    let nets: Vec<DefNet> = (0..wires)
        .map(|i| DefNet {
            name: format!("w{i}"),
            raw_name: String::new(),
            pins: vec![],
            segments: vec![Segment::wire(
                "met1",
                0.0,
                i as f64 * pitch,
                len,
                i as f64 * pitch,
            )],
            vias: 0,
            via_points: Vec::new(),
        })
        .collect();
    let build_ms = t0.elapsed().as_secs_f64() * 1e3;

    let neighbours_per_side = (cutoff / pitch).floor() as usize;
    let expected_pairs = wires.saturating_sub(1) * neighbours_per_side; // rough upper bound

    let t1 = Instant::now();
    let cc = extract_coupling(&nets, &rules, &Default::default(), &Default::default());
    let scan_ms = t1.elapsed().as_secs_f64() * 1e3;

    let pairs = cc.len();
    let total_cap: f64 = cc.iter().map(|c| c.cap_ff).sum();
    eprintln!(
        "coupling_bench: wires={wires} pitch={pitch}um cutoff={cutoff}um \
         (~{neighbours_per_side} neighbours/side)\n\
         \tbuild={build_ms:.1}ms  scan={scan_ms:.1}ms\n\
         \tdistinct_pairs={pairs} (rough_upper_bound={expected_pairs})  total_coupling={total_cap:.3}fF\n\
         \tthroughput={:.2}M pairs/s",
        (pairs as f64 / (scan_ms / 1e3)) / 1e6
    );

    // Sanity: the scan produced coupling and did not silently return nothing.
    assert!(pairs > 0, "dense block must produce coupling pairs");
}
