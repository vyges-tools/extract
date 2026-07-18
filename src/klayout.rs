//! KLayout → SPEF front end. Drives a headless-KLayout net + geometry dumper
//! (`klayout/klayout_netdump.py`) over a GDS, parses its neutral net-dump with the
//! loom KLayout reader, and renders standard SPEF plus the loom EM geometry sidecar
//! (per-segment layer/width/length) that current-density sign-off needs.
//!
//! The KLayout coupling is a *subprocess + data* boundary: the Python driver only
//! dumps geometry/connectivity; all parsing and SPEF serialization live in loom.
//! An offline `self_test` (and a `--from-dump` path) exercise the whole
//! parse→write→re-read pipeline without KLayout, so CI stays hermetic.

use std::process::Command;

use crate::hookup::PinResolver;
use vyges_loom::def::Def;
use vyges_loom::emgeom::EmGeom;
use vyges_loom::klayout as kl;
use vyges_loom::spef::{PinConn, Spef, WriteOpts};

/// The embedded driver — written to a temp path when the caller doesn't point at
/// one on disk, so the tool is self-contained.
pub const DRIVER_PY: &str = include_str!("../klayout/klayout_netdump.py");

#[derive(Debug, Clone, Default)]
pub struct KlOpts {
    pub gds: String,
    pub top: String,
    pub layermap: String,
    pub design: String,
    pub routing_only: bool,
    /// Python invocation, already split (default `["python3"]`). A container
    /// runner is just a longer prefix, e.g.
    /// `["podman","run","--rm","-v",".:.","-w",".","IMAGE","python3"]`.
    pub python: Vec<String>,
    /// Explicit driver path; when `None` the embedded driver is written to a temp.
    pub driver: Option<String>,
    pub version: String,
    pub date: Option<String>,
    /// Std-cell pin/device hookup sources (optional). DEF supplies placement +
    /// net→(instance,pin); cell LEF supplies pin DIRECTION; liberty supplies
    /// per-load input capacitance. When absent, SPEF carries wire RC only.
    pub def: Option<String>,
    pub cell_lef: Option<String>,
    pub lib: Option<String>,
}

pub struct KlResult {
    pub spef: String,
    pub geom: String,
    pub n_nets: usize,
    pub n_segs: usize,
    pub n_pins: usize,
    pub total_cap_ff: f64,
}

/// Attach std-cell pin/device hookup to an RC-only Spef: for each DEF net, add a
/// `*CONN` entry per (instance,pin) with direction (cell LEF MACRO, else liberty)
/// and, for loads, the liberty input capacitance — added to the net's total cap.
/// Returns the number of pins hooked up. Net names must match between the SPEF
/// (KLayout labels) and the DEF.
pub fn attach_hookup(spef: &mut Spef, def: &Def, resolver: &PinResolver) -> usize {
    let mut hooked = 0;
    for dnet in &def.nets {
        let Some(rc) = spef.nets.get_mut(&dnet.name) else {
            continue;
        };
        rc.conns.clear();
        let mut cin_sum = 0.0;
        for (inst, pin) in &dnet.pins {
            let (dir, cap_ff) = resolver.resolve(inst, pin);
            cin_sum += cap_ff;
            rc.conns.push(PinConn {
                inst: inst.clone(),
                pin: pin.clone(),
                node: format!("{inst}:{pin}"),
                dir,
                cap_ff,
            });
            hooked += 1;
        }
        rc.cap_ff += cin_sum; // net cap now includes the load pin caps
    }
    hooked
}

fn render(
    spef: &Spef,
    geom: &EmGeom,
    design: &str,
    version: &str,
    date: Option<String>,
) -> KlResult {
    let n_nets = spef.nets.len();
    let n_segs = geom.segs.len();
    let n_pins = spef.nets.values().map(|n| n.conns.len()).sum();
    let total_cap_ff = spef.nets.values().map(|n| n.cap_ff).sum();
    let opts = WriteOpts {
        design: design.to_string(),
        program: "vyges-extract".into(),
        version: version.to_string(),
        date,
    };
    KlResult {
        spef: spef.to_spef(&opts),
        geom: geom.to_text(),
        n_nets,
        n_segs,
        n_pins,
        total_cap_ff,
    }
}

/// Resolve the driver path: the caller's `--driver`, else materialize the embedded
/// script next to it (deterministic name so a container mount can see it).
fn resolve_driver(opts: &KlOpts) -> Result<String, String> {
    if let Some(p) = &opts.driver {
        return Ok(p.clone());
    }
    let dir = std::path::Path::new(&opts.gds)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let path = dir.join(".vyges-klayout-netdump.py");
    std::fs::write(&path, DRIVER_PY)
        .map_err(|e| format!("cannot write driver to {}: {e}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Run the KLayout driver and return its net-dump text.
pub fn run_driver(opts: &KlOpts) -> Result<String, String> {
    let driver = resolve_driver(opts)?;
    let python = if opts.python.is_empty() {
        vec!["python3".to_string()]
    } else {
        opts.python.clone()
    };
    let (prog, prefix) = python.split_first().unwrap();
    let mut cmd = Command::new(prog);
    cmd.args(prefix);
    cmd.arg(&driver)
        .arg("--gds")
        .arg(&opts.gds)
        .arg("--layermap")
        .arg(&opts.layermap)
        .arg("--out")
        .arg("-")
        .arg("--design")
        .arg(if opts.design.is_empty() {
            "top"
        } else {
            &opts.design
        });
    if !opts.top.is_empty() {
        cmd.arg("--top").arg(&opts.top);
    }
    if opts.routing_only {
        cmd.arg("--routing-only");
    }
    let out = cmd
        .output()
        .map_err(|e| format!("failed to launch KLayout driver ({prog}): {e}"))?;
    // pass the driver's stderr (its [klayout_netdump] log) through to ours
    if !out.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
    }
    if !out.status.success() {
        return Err(format!(
            "KLayout driver exited with {}",
            out.status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into())
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Convert a net-dump into rendered SPEF + geom sidecar text (no pin hookup).
pub fn convert(dump: &str, design: &str, version: &str, date: Option<String>) -> KlResult {
    let (spef, mut geom): (Spef, EmGeom) = kl::parse(dump);
    if geom.design.is_empty() {
        geom.design = design.to_string();
    }
    render(&spef, &geom, design, version, date)
}

/// End-to-end: run KLayout (unless `dump` is supplied) → optional DEF/LEF/liberty
/// pin hookup → SPEF + geom.
pub fn run(opts: &KlOpts, dump: Option<String>) -> Result<KlResult, String> {
    let text = match dump {
        Some(d) => d,
        None => run_driver(opts)?,
    };
    let design = if opts.design.is_empty() {
        "top"
    } else {
        &opts.design
    };
    let (mut spef, mut geom): (Spef, EmGeom) = kl::parse(&text);
    if geom.design.is_empty() {
        geom.design = design.to_string();
    }
    if let Some(def_path) = &opts.def {
        let def = Def::load(def_path).map_err(|e| format!("{def_path}: {e}"))?;
        let resolver = PinResolver::new(&def, opts.cell_lef.as_deref(), opts.lib.as_deref())?;
        attach_hookup(&mut spef, &def, &resolver);
    }
    Ok(render(
        &spef,
        &geom,
        design,
        &opts.version,
        opts.date.clone(),
    ))
}

/// Offline pipeline check — no KLayout. Builds a synthetic net-dump, runs it
/// through the loom reader → SPEF writer → SPEF reader, and asserts the geometry
/// and RC survive. Returns a human-readable summary line.
pub fn self_test() -> Result<String, String> {
    const DUMP: &str = "\
# vyges-klayout-netdump v1
DESIGN selftest
NET clk 14
PIN clk P B
SEG clk clk^met1 350 met1 0.14 12
SEG clk^met1 clk^met2 40 met2 0.2 4
GCAP clk 14
NET dat 5
SEG dat dat^met1 80 met1 0.14 3
GCAP dat 5
";
    let res = convert(DUMP, "selftest", "0", None);
    if res.n_nets != 2 {
        return Err(format!("expected 2 nets, got {}", res.n_nets));
    }
    if res.n_segs != 3 {
        return Err(format!("expected 3 segments, got {}", res.n_segs));
    }
    // SPEF must round-trip through the reader.
    let back = Spef::parse(&res.spef);
    let clk = back
        .nets
        .get("clk")
        .ok_or("clk net lost in SPEF round-trip")?;
    if (clk.res_ohm - 390.0).abs() > 1e-6 {
        return Err(format!("clk res_ohm {} != 390", clk.res_ohm));
    }
    // Geom sidecar must round-trip and carry width/layer.
    let g = EmGeom::parse(&res.geom);
    let m1 = g
        .segs
        .iter()
        .find(|s| s.net == "clk" && s.layer == "met1")
        .ok_or("clk met1 segment lost in geom round-trip")?;
    if (m1.width_um - 0.14).abs() > 1e-9 {
        return Err(format!("clk met1 width {} != 0.14", m1.width_um));
    }
    Ok(format!(
        "self-test OK — {} nets, {} segments, {:.1} fF; SPEF+geom round-trip verified",
        res.n_nets, res.n_segs, res.total_cap_ff
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_self_test() {
        let s = self_test().expect("self-test passes");
        assert!(s.contains("round-trip verified"));
    }

    #[test]
    fn convert_from_dump() {
        let dump = "DESIGN d\nNET n0 3\nSEG n0 n0^met1 100 met1 0.1 5\nGCAP n0 3\n";
        let r = convert(dump, "d", "1", None);
        assert_eq!(r.n_nets, 1);
        assert_eq!(r.n_segs, 1);
        assert!(r.spef.contains("*D_NET"));
        // geom sidecar line: SEG <net> <a> <b> <layer> <w> <l> <res>
        assert!(
            r.geom.contains("SEG n0 n0 n0^met1 met1 0.1 5 100"),
            "geom was:\n{}",
            r.geom
        );
    }

    #[test]
    fn hookup_marks_driver_load_and_cin() {
        // RC-only Spef from KLayout
        let (mut spef, _) =
            kl::parse("DESIGN t\nNET clk 2.0\nSEG clk clk^met1 100 met1 0.14 5\nGCAP clk 2.0\n");
        let def = Def::parse(
            "VERSION 5.8 ;\nDESIGN t ;\nUNITS DISTANCE MICRONS 1000 ;\n\
             COMPONENTS 2 ;\n- u1 INV_X1 + PLACED ( 0 0 ) N ;\n- u2 INV_X1 + PLACED ( 1000 0 ) N ;\nEND COMPONENTS\n\
             NETS 1 ;\n- clk ( u1 Y ) ( u2 A ) ;\nEND NETS\nEND DESIGN\n",
        )
        .unwrap();
        let cell_lef = vyges_loom::lef::Lef::parse(
            "MACRO INV_X1\n PIN A\n  DIRECTION INPUT ;\n END A\n PIN Y\n  DIRECTION OUTPUT ;\n END Y\nEND INV_X1\n",
        )
        .unwrap();
        let lib = vyges_loom::liberty::Lib::parse(
            "library(t) {\n  capacitive_load_unit (1, ff);\n  cell (INV_X1) {\n    pin(A) { direction: input; capacitance: 1.2; }\n    pin(Y) { direction: output; }\n  }\n}\n",
        )
        .unwrap();

        let resolver = PinResolver::from_loaded(&def, Some(cell_lef), Some(lib));
        let hooked = attach_hookup(&mut spef, &def, &resolver);
        assert_eq!(hooked, 2);
        let rc = spef.nets.get("clk").unwrap();
        let y = rc.conns.iter().find(|c| c.pin == "Y").unwrap();
        let a = rc.conns.iter().find(|c| c.pin == "A").unwrap();
        use vyges_loom::lef::PinDir;
        assert_eq!(y.dir, PinDir::Output);
        assert_eq!(a.dir, PinDir::Input);
        assert!((a.cap_ff - 1.2).abs() < 1e-6, "Cin was {}", a.cap_ff);
        assert!(
            (rc.cap_ff - 3.2).abs() < 1e-6,
            "net cap should be 2.0 + 1.2 Cin, was {}",
            rc.cap_ff
        );

        // and it renders standard *CONN with O/I + *L
        let out = render(
            &spef,
            &vyges_loom::emgeom::EmGeom::default(),
            "t",
            "0",
            None,
        );
        assert!(
            out.spef.contains(" O\n") && out.spef.contains(" I *L 1.2"),
            "spef:\n{}",
            out.spef
        );
        assert_eq!(out.n_pins, 2);
    }
}
