//! End-to-end: the example job runs offline (v0 is pure-std, no subprocess).

use vyges_extract::engine::run_to_spef;
use vyges_extract::job::ExtractJob;

#[test]
fn example_counter_extracts_to_spef() {
    let job_path = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/counter/counter.ext");
    let job = ExtractJob::load(job_path).unwrap();
    let spef = run_to_spef(&job).unwrap();

    assert!(spef.contains("*DESIGN \"counter\""));
    assert!(spef.contains("*1 clk"));
    // clk/n0 met1 verticals run parallel: centerline gap 1.0um; LEF met1 width
    // 0.14um -> edge gap 0.86um; overlap 0.8um; coupling 0.050 fF/um at s_ref
    // 0.14um -> Cc = 0.050 * 0.8 * (0.14/0.86) = 0.006512 fF.
    // clk total = ground 0.45 + coupling 0.006512 = 0.456512
    assert!(spef.contains("*D_NET *1 0.456512"), "clk total\n{spef}");
    // per-pin RC tree: driver clkbuf (node 3) near-half 0.225, two sinks share
    // the far half (0.1125 each); trunk R/2, branches R/4 from the net root.
    assert!(spef.contains("*CAP\n1 *3:X 0.225000"), "driver near cap\n{spef}");
    assert!(spef.contains("2 *4:CLK 0.112500"), "sink cap share\n{spef}");
    assert!(spef.contains("1 *1 *3:X 5.025000"), "trunk R/2\n{spef}");
    assert!(spef.contains(" *1 *2 0.006512"), "coupling cap clk-n0\n{spef}");
    // n0 total = ground 0.0804 + coupling 0.006512 = 0.086912
    assert!(spef.contains("*D_NET *2 0.086912"), "n0 total");
    // n0 (driver u0 + 1 sink u1): R 3.94 split into trunk + branch, 1.97 each
    assert!(spef.contains(" *2 *6:Y 1.970000"), "n0 trunk\n{spef}");
}
