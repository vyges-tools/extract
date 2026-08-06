//! **Can a real timer read the SPEF we write?**
//!
//! Everything else here checks the numbers. This checks that the file is usable at all — which
//! is a separate property, and the one that fails silently. A SPEF whose names a timer cannot
//! resolve does not error: it annotates the nets it recognises, skips the rest with a warning
//! nobody reads, and reports a design faster than it is.
//!
//! Our own reader cannot see this. It reads what we write by construction, so a name we spell
//! wrongly round-trips perfectly. Only a reader we did not write can say.
//!
//! **Integrated at the file format, over a pipe** — `sta` is invoked as a binary with a Tcl
//! script, nothing links against it.
//!
//! Opt-in, and needs a hardened design because the interesting names only occur in one:
//!
//! ```sh
//! VYGES_STA=/usr/local/bin/sta \
//! VYGES_CORPUS=~/runs \
//! VYGES_LIB=$PDK/…/sky130_fd_sc_hd__tt_025C_1v80.lib \
//! VYGES_TLEF=$PDK/…/sky130_fd_sc_hd__nom.tlef \
//! VYGES_RULES=~/…/sky130A.vyges-extract.rules \
//!   cargo test --release --test opensta -- --nocapture
//! ```
//!
//! What it found the first time it was run, on the SPEF this engine writes today:
//!
//! - Bus brackets were re-escaped in the name map. `count\[0\]` is legal SPEF but it means a net
//!   whose NAME contains brackets, not bit 0 of bus `count` — and OpenSTA reported
//!   `net count\[0\] not found` for every bussed net in the design. On a design whose DEF names
//!   were already escaped the same code doubled the backslash and the file became a syntax error
//!   at the name map. The DEF already carries the distinction; the fix was to stop re-deriving
//!   it and write what we read.
//! - Capacitance was written to six decimal PLACES, so anything below 5e-7 fF became `0.000000`
//!   — the capacitor deleted, in a file that still parses.
//! - Coupling capacitors were listed under net A only. A reader applies one to the net whose
//!   block it appears in, so B never learned it was coupled.

use std::path::{Path, PathBuf};
use std::process::Command;

use vyges_extract::engine::run_to_spef;
use vyges_extract::job::ExtractJob;

fn env_file(k: &str) -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var(k).ok()?);
    p.is_file().then_some(p)
}

fn sta_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VYGES_STA") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let out = Command::new("sh").arg("-c").arg("command -v sta").output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// A hardened design in the LibreLane `final/` layout: the routed DEF we extract from, plus the
/// netlist and constraints the timer needs to make sense of the result.
struct Design {
    top: String,
    def: PathBuf,
    netlist: PathBuf,
    sdc: Option<PathBuf>,
}

fn first_with(dir: &Path, ext: &str) -> Option<PathBuf> {
    let mut hits: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_str().is_some_and(|s| s.ends_with(ext)))
        .collect();
    hits.sort();
    hits.into_iter().next()
}

fn designs(root: &Path) -> Vec<Design> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            if p.file_name().and_then(|s| s.to_str()) == Some("final") {
                let (Some(def), Some(netlist)) = (
                    first_with(&p.join("def"), ".def"),
                    first_with(&p.join("nl"), ".nl.v").or_else(|| first_with(&p.join("nl"), ".v")),
                ) else {
                    continue;
                };
                let top = netlist
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.split('.').next().unwrap_or(s).to_string())
                    .unwrap_or_default();
                out.push(Design { top, def, netlist, sdc: first_with(&p.join("sdc"), ".sdc") });
            } else {
                stack.push(p);
            }
        }
    }
    out.sort_by(|a, b| a.top.cmp(&b.top));
    out
}

#[test]
fn opensta_can_read_the_spef_this_engine_writes() {
    let (Some(sta), Some(lib), Some(tlef), Some(rules)) =
        (sta_binary(), env_file("VYGES_LIB"), env_file("VYGES_TLEF"), env_file("VYGES_RULES"))
    else {
        println!(
            "OpenSTA: skipped — needs VYGES_STA (or `sta` on PATH), VYGES_LIB, VYGES_TLEF and \
             VYGES_RULES, and VYGES_CORPUS pointing at a tree of hardened runs"
        );
        return;
    };
    let Ok(root) = std::env::var("VYGES_CORPUS") else {
        println!("OpenSTA: skipped — set VYGES_CORPUS to a tree of hardened runs");
        return;
    };
    let designs = designs(Path::new(&root));
    println!("OpenSTA: {} hardened design(s)", designs.len());

    let work = std::env::temp_dir().join(format!("vyges-extract-opensta-{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("work dir");
    let mut bad = Vec::new();

    for d in &designs {
        // Extract, through the same path the CLI uses.
        let job_path = work.join(format!("{}.ext", d.top));
        std::fs::write(
            &job_path,
            format!(
                "design:   {}\ndef:      {}\nrules:    {}\nlef:      {}\ncorner:   typical\ntemp:     25\n",
                d.top,
                d.def.display(),
                rules.display(),
                tlef.display()
            ),
        )
        .expect("write job");
        let Ok(job) = ExtractJob::load(job_path.to_str().unwrap()) else {
            bad.push(format!("{}: job would not load", d.top));
            continue;
        };
        let Ok(spef) = run_to_spef(&job) else {
            bad.push(format!("{}: extraction failed", d.top));
            continue;
        };
        let spef_path = work.join(format!("{}.spef", d.top));
        std::fs::write(&spef_path, &spef).expect("write spef");

        // Read it back with a timer that is not ours.
        let sdc = match &d.sdc {
            Some(p) => format!("read_sdc {}\n", p.display()),
            None => String::new(),
        };
        let script = work.join("run.tcl");
        std::fs::write(
            &script,
            format!(
                "read_liberty {}\nread_verilog {}\nlink_design {}\n{sdc}read_spef {}\nexit\n",
                lib.display(),
                d.netlist.display(),
                d.top,
                spef_path.display()
            ),
        )
        .expect("write tcl");
        let out = Command::new(&sta).arg("-no_splash").arg("-exit").arg(&script).output();
        let Ok(out) = out else {
            bad.push(format!("{}: could not run sta", d.top));
            continue;
        };
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        // A design whose netlist instantiates a hardened MACRO cannot be checked with only the
        // standard-cell Liberty: OpenSTA black-boxes the macro, infers its pins from the
        // instantiation, and then cannot confirm which net each one is on — so it objects to
        // every macro pin in the file whatever the file says. That is a gap in what this check
        // was given, not a defect in what we wrote, and it is NAMED rather than dropped
        // quietly. (Fill and tap cells are black-boxed in every design and are not macros.)
        let macro_boxed: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("not found. Creating black box"))
            .filter(|l| !l.contains("sky130_"))
            .collect();
        if !macro_boxed.is_empty() {
            println!(
                "  --  {} skipped: {} macro(s) black-boxed for want of their Liberty, e.g. {}",
                d.top,
                macro_boxed.len(),
                macro_boxed[0].split_whitespace().nth(5).unwrap_or("?")
            );
            continue;
        }
        // Only complaints about the PARASITICS. A netlist that instantiates fill and tap cells
        // the Liberty does not define warns whatever SPEF it is given, and is not ours.
        let name = spef_path.file_name().and_then(|s| s.to_str()).unwrap_or("spef");
        let complaints: Vec<&str> = text
            .lines()
            .filter(|l| (l.starts_with("Error:") || l.starts_with("Warning:")) && l.contains(name))
            .collect();
        if complaints.is_empty() {
            println!("  ok  {} ({} nets)", d.top, spef.matches("*D_NET").count());
        } else {
            bad.push(format!(
                "{}: OpenSTA objected to our SPEF ({} of them):\n    {}",
                d.top,
                complaints.len(),
                complaints.iter().take(5).cloned().collect::<Vec<_>>().join("\n    ")
            ));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    let _ = std::fs::remove_dir_all(&work);
}
