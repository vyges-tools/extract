//! End-to-end: the example job runs offline (v0 is pure-std, no subprocess).

use vyges_extract::engine::run_to_spef;
use vyges_extract::job::ExtractJob;

/// Sum the single-node `*CAP` grounded entries and the `*RES` resistances inside
/// one net's `*D_NET … *END` block, and report whether any internal junction node
/// (`*<netid>:<k>`) appears — i.e. the net was emitted as a distributed tree.
fn net_block(spef: &str, dnet_id: &str) -> (f64, f64, bool) {
    let start = spef
        .find(&format!("*D_NET {dnet_id} "))
        .expect("net present");
    let block = &spef[start..start + spef[start..].find("*END").unwrap()];
    let mut cap = 0.0;
    let mut res = 0.0;
    let mut internal = false;
    let (mut in_cap, mut in_res) = (false, false);
    let want_internal = format!("*{}:", dnet_id.trim_start_matches('*'));
    for line in block.lines() {
        match line.trim() {
            "*CAP" => {
                in_cap = true;
                in_res = false;
                continue;
            }
            "*RES" => {
                in_res = true;
                in_cap = false;
                continue;
            }
            l if l.starts_with('*') => {
                in_cap = false;
                in_res = false;
            }
            _ => {}
        }
        let tok: Vec<&str> = line.split_whitespace().collect();
        if in_cap && tok.len() == 3 {
            // "idx *node value" — single-node grounded cap (coupling has 4 tokens)
            cap += tok[2].parse::<f64>().unwrap_or(0.0);
        }
        if in_res && tok.len() == 4 {
            res += tok[3].parse::<f64>().unwrap_or(0.0);
        }
        if line.contains(&want_internal) {
            internal = true;
        }
    }
    (cap, res, internal)
}

#[test]
fn example_counter_extracts_to_spef() {
    let job_path = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/counter/counter.ext");
    let job = ExtractJob::load(job_path).unwrap();
    let spef = run_to_spef(&job).unwrap();

    assert!(spef.contains("*DESIGN \"counter\""));
    assert!(spef.contains("*1 clk"));

    // Totals are unchanged by the distributed model — it redistributes the same
    // calibrated per-net R/C over the real routing, it does not re-extract them.
    // clk total = ground 0.45 + coupling 0.006512 = 0.456512.
    assert!(spef.contains("*D_NET *1 0.456512"), "clk total\n{spef}");
    // n0 total = ground 0.0804 + coupling 0.006512 = 0.086912.
    assert!(spef.contains("*D_NET *2 0.086912"), "n0 total\n{spef}");
    // coupling listed once as a two-node entry with the same value as before.
    assert!(
        spef.lines()
            .any(|l| l.contains("0.006512") && l.matches('*').count() == 2),
        "coupling clk-n0 two-node entry\n{spef}"
    );

    // clk is emitted as a DISTRIBUTED tree: it has an internal wire-junction node
    // and its node caps / edge resistances reconcile to the lumped totals (0.45 fF
    // grounded, 10.05 ohm — met1 + met2 runs + the M1M2 via).
    let (cap, res, internal) = net_block(&spef, "*1");
    assert!(
        internal,
        "clk should expose an internal junction node\n{spef}"
    );
    assert!(
        (cap - 0.45).abs() < 1e-6,
        "clk grounded cap sums to 0.45, got {cap}"
    );
    assert!(
        (res - 10.05).abs() < 1e-6,
        "clk resistances sum to 10.05, got {res}"
    );
    assert!(
        spef.contains("9.300000"),
        "the M1M2 via resistor is present\n{spef}"
    );

    // n0 likewise distributed and reconciled (ground 0.0804, R 3.94).
    let (cap2, res2, internal2) = net_block(&spef, "*2");
    assert!(internal2, "n0 should expose an internal junction node");
    assert!(
        (cap2 - 0.0804).abs() < 1e-6,
        "n0 grounded cap sums to 0.0804, got {cap2}"
    );
    assert!(
        (res2 - 3.94).abs() < 1e-6,
        "n0 resistances sum to 3.94, got {res2}"
    );
}
