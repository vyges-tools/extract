//! `run` must not silently extract against a ruleset with no R/C.
//!
//! Zero parasitics are not a measurement, they are a missing input. A deck derived from a
//! geometry-only tech LEF says every wire is a perfect conductor, and the resulting SPEF looks
//! like a clean result rather than an absent captable. These tests pin the refusal, the
//! explicit override, and — most importantly — that a *complete* deck is unaffected, since a
//! gate that fires on good input is worse than no gate.

use std::path::{Path, PathBuf};
use std::process::Command;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The counter example, whose sky130 deck is fully specified.
fn counter_job() -> PathBuf {
    manifest().join("examples/counter/counter.ext")
}

/// Write a job whose rules are `examples/counter/sky130.rules` with every R/C zeroed.
///
/// The layer *names* are kept, so the only difference from the working example is the missing
/// numbers. Renaming them would make extraction fail with "no rule for layer met1" and the test
/// would pass for the wrong reason — which is exactly what happened when this was first written
/// by hand with invented layer names.
fn zeroed_job(dir: &Path) -> PathBuf {
    let real = std::fs::read_to_string(manifest().join("examples/counter/sky130.rules"))
        .expect("read sky130 rules");
    let mut deck = String::new();
    for line in real.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        match t.first() {
            // a layer row: keep the name and the reference spacing, zero the R/C
            Some(n) if !line.starts_with('#') && t.len() >= 5 && !is_keyword(n) => {
                deck.push_str(&format!("{n} 0 0 0 {}\n", t[4]));
            }
            Some(_) if !line.starts_with('#') => deck.push_str(&format!("{line}\n")),
            _ => {}
        }
    }
    let rules = dir.join("zeroed.rules");
    std::fs::write(&rules, deck).expect("write zeroed rules");
    let job = dir.join("zeroed.ext");
    std::fs::write(
        &job,
        format!(
            "design: counter\ndef: {}\nrules: {}\n",
            manifest().join("examples/counter/counter.def").display(),
            rules.display()
        ),
    )
    .expect("write job");
    job
}

fn is_keyword(t: &str) -> bool {
    matches!(
        t,
        "via" | "couple_cutoff" | "rsheet" | "height" | "eps_r" | "shield_k" | "interlayer"
    )
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vyges-extract"))
        .args(args)
        .output()
        .expect("spawn vyges-extract")
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "vyges-extract-rc-gate-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

#[test]
fn run_refuses_a_deck_with_no_r_or_c() {
    let dir = tmpdir("refuse");
    let job = zeroed_job(&dir);
    let out = run(&[
        "run",
        job.to_str().unwrap(),
        "-o",
        dir.join("out.spef").to_str().unwrap(),
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "a zero-R/C deck must be a usage error, not a silent extraction.\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    // The message has to name the remedy, not just the complaint.
    assert!(err.contains("--captable"), "should point at the fix: {err}");
    assert!(
        err.contains("--allow-incomplete-rc"),
        "should offer the override: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_override_extracts_anyway() {
    let dir = tmpdir("override");
    let job = zeroed_job(&dir);
    let out = run(&[
        "run",
        job.to_str().unwrap(),
        "--allow-incomplete-rc",
        "-o",
        dir.join("out.spef").to_str().unwrap(),
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "--allow-incomplete-rc must let the extraction proceed.\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Overriding silences nothing: the warning is the whole point of allowing the override.
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no resistance"),
        "the override must still warn"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The case that matters most: a gate that fires on a good deck would be worse than no gate.
#[test]
fn a_complete_deck_is_untouched() {
    let dir = tmpdir("clean");
    let out = run(&[
        "run",
        counter_job().to_str().unwrap(),
        "-o",
        dir.join("out.spef").to_str().unwrap(),
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the fully-specified sky130 deck must extract normally.\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains("no resistance") && !err.contains("no capacitance"),
        "no incompleteness warning belongs on a complete deck: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
