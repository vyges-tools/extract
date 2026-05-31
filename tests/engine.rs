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
    assert!(spef.contains("*CAP\n1 *1 0.450000")); // grounded portion
    assert!(spef.contains("2 *1 *2 0.006512"), "coupling cap clk-n0\n{spef}");
    // n0 total = ground 0.0804 + coupling 0.006512 = 0.086912
    assert!(spef.contains("*D_NET *2 0.086912"), "n0 total");
    assert!(spef.contains("10.050000")); // clk res
    assert!(spef.contains("3.940000")); // n0 res
}
