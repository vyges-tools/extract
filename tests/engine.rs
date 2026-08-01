//! End-to-end: the example job runs offline (v0 is pure-std, no subprocess).

use vyges_extract::engine::run_to_spef;
use vyges_extract::job::ExtractJob;

/// Assert every `*D_NET` in a rendered SPEF is **one** connected RC network.
///
/// This is the invariant OpenRCX checks as `RCX-0272 RC of net … is disconnected`, and on one
/// real block it was false for 7 % of nets — sinks with no resistive path to their driver, which
/// a timer reads as no interconnect delay at all. It is checkable entirely from our own output,
/// so its absence was ours: nothing here needed OpenROAD to catch it.
///
/// Deliberately a check on the *file* rather than on `RcNetwork`, because the star fallback and
/// the emitter are between the builder and what a reader actually sees.
fn assert_every_net_is_one_network(spef: &str) {
    use std::collections::{BTreeMap, BTreeSet};
    let mut checked = 0;
    for block in spef.split("*D_NET ").skip(1) {
        let name = block.split_whitespace().next().unwrap_or("?").to_string();
        let body = &block[..block.find("*END").unwrap_or(block.len())];
        // grounded `*CAP` entries (2 fields after the index) are the nodes; `*RES` are the edges
        let mut nodes: BTreeSet<&str> = BTreeSet::new();
        let mut edges: Vec<(&str, &str)> = Vec::new();
        let (mut in_cap, mut in_res) = (false, false);
        for line in body.lines() {
            match line.trim() {
                "*CAP" => (in_cap, in_res) = (true, false),
                "*RES" => (in_cap, in_res) = (false, true),
                "*CONN" => (in_cap, in_res) = (false, false),
                l => {
                    let f: Vec<&str> = l.split_whitespace().collect();
                    if in_cap && f.len() == 3 {
                        nodes.insert(f[1]); // coupling entries have 4 fields — not this net's
                    } else if in_res && f.len() == 4 {
                        nodes.insert(f[1]);
                        nodes.insert(f[2]);
                        edges.push((f[1], f[2]));
                    }
                }
            }
        }
        if nodes.len() < 2 {
            continue; // a single lumped node is trivially one network
        }
        let idx: BTreeMap<&str, usize> = nodes.iter().copied().zip(0..).collect();
        let mut parent: Vec<usize> = (0..idx.len()).collect();
        fn find(parent: &mut [usize], mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        for (a, b) in edges {
            let (ra, rb) = (find(&mut parent, idx[a]), find(&mut parent, idx[b]));
            parent[ra] = rb;
        }
        let pieces = (0..idx.len())
            .filter(|&i| find(&mut parent, i) == i)
            .count();
        assert_eq!(
            pieces,
            1,
            "net {name}: {} nodes in {pieces} disconnected pieces — some node has no \
             resistive path to the rest\n{body}",
            idx.len()
        );
        checked += 1;
    }
    assert!(checked > 0, "the scan found no multi-node net to check");
}

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

    assert_every_net_is_one_network(&spef);
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

/// The connectivity check has to be able to fail, or it is decoration.
///
/// Same shape the real defect had: a `*CAP` node that no `*RES` edge reaches.
#[test]
#[should_panic(expected = "disconnected pieces")]
fn the_connectivity_check_rejects_a_net_in_pieces() {
    assert_every_net_is_one_network(
        "*D_NET *1 1.0\n\
         *CONN\n\
         *I *2:Y O\n\
         *I *3:A I\n\
         *CAP\n\
         1 *2:Y 0.4\n\
         2 *1:1 0.3\n\
         3 *3:A 0.3\n\
         *RES\n\
         1 *2:Y *1:1 5.0\n\
         *END\n",
    );
}

/// …and pass the same net once the missing edge is there.
#[test]
fn the_connectivity_check_accepts_the_repaired_net() {
    assert_every_net_is_one_network(
        "*D_NET *1 1.0\n\
         *CONN\n\
         *I *2:Y O\n\
         *I *3:A I\n\
         *CAP\n\
         1 *2:Y 0.4\n\
         2 *1:1 0.3\n\
         3 *3:A 0.3\n\
         *RES\n\
         1 *2:Y *1:1 5.0\n\
         2 *1:1 *3:A 5.0\n\
         *END\n",
    );
}

/// `shield_k` must actually be **applied**, not merely parsed.
///
/// The deck ships `shield_k 0.18081` and the whole ground calibration is conditioned on that
/// subtraction happening. Until this test there was one covering the *parser* — carrying a
/// comment noting that application lives in the engine — and nothing covering the engine. That
/// is the same shape of gap that let OpenROAD's `diff_spef` rot: a code path with a plausible
/// test next to it that does not exercise it.
#[test]
fn shield_k_is_applied_to_the_grounded_cap_not_just_parsed() {
    use std::fs;
    use vyges_extract::engine::extract;

    let dir = std::env::temp_dir().join("vyges_extract_shield_applied");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    // Two parallel met1 wires, close enough to couple: one net pair, one coupling cap.
    fs::write(
        dir.join("s.def"),
        "VERSION 5.8 ;\nDESIGN s ;\nUNITS DISTANCE MICRONS 1000 ;\n\
         COMPONENTS 0 ;\nEND COMPONENTS\n\
         NETS 2 ;\n\
         - a ( u0 Y ) ( u1 A )\n  + ROUTED met1 ( 0 0 ) ( 10000 0 ) ;\n\
         - b ( u2 Y ) ( u3 A )\n  + ROUTED met1 ( 0 340 ) ( 10000 340 ) ;\n\
         END NETS\nEND DESIGN\n",
    )
    .unwrap();
    let deck =
        |k: f64| format!("met1 0.125 0.1 0.08 0.14\nvia 9.3\ncouple_cutoff 2.0\nshield_k {k}\n");

    let run = |k: f64| {
        fs::write(dir.join("s.rules"), deck(k)).unwrap();
        fs::write(
            dir.join("s.ext"),
            "design: s\ndef: s.def\nrules: s.rules\ncorner: typical\ntemp: 25\n",
        )
        .unwrap();
        let job = ExtractJob::load(dir.join("s.ext").to_str().unwrap()).unwrap();
        let ex = extract(&job).unwrap();
        let g: f64 = ex.nets.iter().map(|n| n.cap_ff).sum();
        let c: f64 = ex.couplings.iter().map(|c| c.cap_ff).sum();
        (g, c)
    };

    let (g0, c0) = run(0.0);
    let (g5, c5) = run(0.5);
    assert!(
        c0 > 0.0,
        "the fixture must actually couple, else this proves nothing"
    );
    assert!(
        (c5 - c0).abs() < 1e-9,
        "shielding moves charge out of ground, it must not change the coupling itself"
    );
    // Each net is charged the full pair cap (both endpoints), so the total drop is k * 2 * Cc.
    let expected = g0 - 0.5 * 2.0 * c0;
    assert!(
        (g5 - expected).abs() < 1e-6,
        "ground with shield_k=0.5 should be {expected:.6} fF, got {g5:.6} (unshielded {g0:.6})"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Guard the shipped deck against the failure modes the calibration scripts guard against
/// internally — a hand edit does not go through those scripts.
///
/// A least-squares fit returns 0.0 for a layer nothing constrains, and writing that silently
/// switches the layer off. The scripts refuse it; nothing stopped a person doing it by hand.
#[test]
fn the_shipped_sky130a_deck_stays_sane() {
    use vyges_extract::rules::RcRules;
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/pdk/sky130A/sky130A.vyges-extract.rules"
    );
    let r = RcRules::load(path).expect("the shipped deck must parse");

    assert!(!r.layers.is_empty(), "deck has layers");
    for (name, l) in &r.layers {
        assert!(
            l.res_per_um > 0.0,
            "{name}: zero resistance would report no R at all"
        );
        assert!(
            l.cap_per_um > 0.0,
            "{name}: zero ground cap would report no C at all"
        );
        assert!(
            l.coupling_per_um > 0.0,
            "{name}: zero coupling — this is what an unconstrained least-squares fit returns, \
             and it silently removes the layer from every SI calculation downstream"
        );
    }
    // Shielding is load-bearing for the ground calibration; a fraction outside (0,1) is not a
    // charge-conservation share of anything.
    assert!(
        r.shield_k > 0.0 && r.shield_k < 1.0,
        "shield_k = {} — the ground fit assumes it is a fraction and that it is ON",
        r.shield_k
    );
}
