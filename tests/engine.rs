//! End-to-end: the example job runs offline (v0 is pure-std, no subprocess).

use vyges_extract::engine::run_to_spef;
use vyges_extract::job::ExtractJob;

#[test]
fn example_counter_extracts_to_spef() {
    let job_path = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/counter/counter.ext");
    let job = ExtractJob::load(job_path).unwrap();
    let spef = run_to_spef(&job).unwrap();

    assert!(spef.contains("*DESIGN \"counter\""));
    // clk: R = 3*0.125 + 3*0.125 + 1*9.3 = 10.05 ; C = 3*0.078 + 3*0.072 = 0.45
    assert!(spef.contains("*1 clk"));
    assert!(spef.contains("*D_NET *1 0.450000"));
    assert!(spef.contains("10.050000"));
    // n0: R = 0.8*0.125 + 0.3*12.8 = 3.94 ; C = 0.8*0.078 + 0.3*0.060 = 0.0804
    assert!(spef.contains("*D_NET"));
    assert!(spef.contains("3.940000"));
    assert!(spef.contains("0.080400"));
}
