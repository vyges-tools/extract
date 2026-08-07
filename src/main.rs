//! vyges-extract CLI.
//!
//!   vyges-extract run   JOB [-o OUT] [--json]   extract -> SPEF (or JSON summary)
//!   vyges-extract check JOB                      validate the job + inputs
//!   vyges-extract demo  [-o OUT] [--json]        sample output (no inputs)
//!
//! Common flags: -h/--help, -V/--version, -q/--quiet, -v/--verbose.
//! Exit codes: 0 ok · 1 runtime error · 2 usage/validation.

use std::process::exit;

use vyges_extract::coupling::CouplingCap;
use vyges_extract::engine;
use vyges_extract::job::ExtractJob;
use vyges_extract::rc::NetParasitics;
use vyges_extract::spef::{self, Units};
use vyges_extract::tree::RcNetwork;

const USAGE: &str = "\
vyges-extract — foundry-correlated RC parasitic extraction (DEF -> SPEF)

usage:
  vyges-extract run    JOB [-o OUT] [--json] [--pdk NAME | --tech-lef PATH] [--refresh]
                           [--captable PATH | --allow-incomplete-rc]
                           [--cell-lef CL --lib LIB]   # std-cell *CONN hookup
  vyges-extract gen-rc (--pdk NAME | --tech-lef PATH) [--refresh]
  vyges-extract check  JOB
  vyges-extract demo   [-o OUT] [--json]
  vyges-extract klayout2spef --gds F --layermap M [--top CELL] [-o OUT] [--geom-out G]
                             [--def D --cell-lef CL --lib LIB]
                             [--runner podman [--image REF] [--mount DIR] | --python CMD]
                             [--routing-only] [--from-dump F] [--self-test]

klayout2spef drives a headless KLayout (LayoutToNetlist) over a GDS to write SPEF +
an EM geometry sidecar (per-segment layer/width/length) for current-density sign-off.
--runner podman wraps the driver in the vyges-klayout container; --self-test and
--from-dump run the parse→SPEF pipeline offline (no KLayout).

RC rules come from a job's `rules:`, or are DERIVED from the PDK tech LEF via
--pdk / --tech-lef (the metal stack is discovered, not hand-listed) and cached
as vyges-additions/<pdk>/vyges-extract-rc.rules (regenerate with --refresh). When
a tech LEF is geometry-only (no R/C), an OpenRCX captable supplies the numbers;
`run` refuses zero-R/C rules rather than report parasitics that are all zero.

flags:
  --pdk NAME       derive RC rules from the PDK tech LEF (resolved via pdk-store)
  --tech-lef PATH  derive RC rules from this tech LEF directly
  --captable PATH  OpenRCX rules file for R/C the LEF lacks (else pdk-store captable)
  --refresh        re-derive the cached RC rules
  --allow-incomplete-rc  extract even when a layer has no R/C (understates parasitics)
  -o FILE          write output to FILE (default: stdout)
  --json           per-net parasitics summary as JSON instead of SPEF
  -q, --quiet      suppress non-essential output
  -v, --verbose    extra detail on stderr
  -j, --threads N  parallel worker threads (default: all cores; 1 = serial)
  --describe       print a machine-readable JSON description of the command
  -h, --help       show this help
  -V, --version    show version
  --bug-report     file a bug (central: vyges/community)
  --feature-request request a feature (central)
  --sponsor        sponsor Vyges (github.com/sponsors/vyges-ip)
  --star           star this tool on GitHub ⭐
";

const BUG_URL: &str =
    "https://github.com/vyges/community/issues/new?template=bug_report_template.yaml";
const FEATURE_URL: &str = "https://github.com/vyges/community/issues/new?labels=enhancement";
const SPONSOR_URL: &str = "https://github.com/sponsors/vyges-ip";
const STAR_URL: &str = "https://github.com/vyges-tools/extract";

/// Print a labelled URL; if stdout is a terminal, also try to open it in a browser.
/// In headless / agent contexts (not a TTY) it just prints the URL.
fn link(label: &str, url: &str) {
    use std::io::IsTerminal;
    println!("{label}:\n  {url}");
    if std::io::stdout().is_terminal() {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener).arg(url).status();
    }
}

#[derive(Default)]
struct Cli {
    positionals: Vec<String>,
    out: Option<String>,
    threads: Option<usize>,
    json: bool,
    quiet: bool,
    verbose: bool,
    help: bool,
    version: bool,
    bug_report: bool,
    feature_request: bool,
    sponsor: bool,
    star: bool,
    pdk: Option<String>,
    tech_lef: Option<String>,
    captable: Option<String>,
    refresh: bool,
    allow_incomplete_rc: bool,
    /// Set when this run proceeded over an incomplete RC deck, with the reason.
    incomplete_rc_note: Option<String>,
    // klayout2spef front end
    gds: Option<String>,
    top: Option<String>,
    layermap: Option<String>,
    routing_only: bool,
    geom_out: Option<String>,
    from_dump: Option<String>,
    self_test: bool,
    python: Option<String>,
    runner: Option<String>,
    image: Option<String>,
    mount: Option<String>,
    driver: Option<String>,
    date: Option<String>,
    def: Option<String>,
    cell_lef: Option<String>,
    lib: Option<String>,
}

/// Resolve the RC ruleset for a `--pdk` (or explicit `--tech-lef`): derive per-layer
/// RC from the PDK tech LEF for the whole discovered metal stack, and cache it as
/// `vyges-additions/<pdk>/vyges-extract-rc.rules` next to the PDK's other Vyges
/// collateral (regenerated on `--refresh`). Returns the cache path.
/// Raw path of a PDK collateral key from the installed pdk-store — the path is
/// returned even if the file does not exist (for computing a *write* target such as
/// a cache directory). Prefers the sibling `vyges-pdk-store`, else PATH.
fn pdk_store_raw(pdk: &str, key: &str) -> Option<String> {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("vyges-pdk-store")))
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned());
    let prog = sibling.unwrap_or_else(|| "vyges-pdk-store".into());
    let out = std::process::Command::new(prog)
        .args(["resolve", pdk, key])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Whether a resolved collateral entry is a local absolute path, and so usable as a directory to
/// write into. A URL scheme is the case that matters (`https:`, `s3:`, `git+ssh:`); a relative
/// path is also rejected, because the directory it names depends on where the tool was invoked.
fn is_local_path(s: &str) -> bool {
    // A Windows drive letter (`C:\`) is not a scheme; a scheme is 2+ chars before the colon.
    let scheme = s.split_once(':').map(|(head, _)| head).unwrap_or("");
    let has_scheme = scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    !has_scheme && std::path::Path::new(s).is_absolute()
}

/// Report layers with no resistance or no capacitance. Returns whether anything is missing.
///
/// Called at generation *and* on every load. The cache is the reason: `ensure_rc_rules` only
/// re-derives when the file is absent or `--refresh` is passed, so a warning printed once at
/// generation is invisible on every subsequent run — which is exactly when someone is relying
/// on the numbers.
fn warn_if_incomplete(rules: &vyges_extract::rules::RcRules) -> bool {
    let (no_r, no_c) = rules.incomplete();
    if !no_r.is_empty() {
        eprintln!(
            "warning: no resistance for [{}] — the tech LEF is geometry-only; pass \
             --captable <rcx_rules>",
            no_r.join(", ")
        );
    }
    if !no_c.is_empty() {
        // Only call the deck "resistance-only" when it actually has resistance — otherwise
        // this message contradicts the one above it and misdescribes the deck.
        let why = if no_r.is_empty() {
            "RC is resistance-only"
        } else {
            "the deck carries neither R nor C for these layers"
        };
        eprintln!(
            "warning: no capacitance for [{}] — {why}; pass --captable for cap",
            no_c.join(", ")
        );
    }
    !no_r.is_empty() || !no_c.is_empty()
}

fn ensure_rc_rules(cli: &Cli) -> Result<String, String> {
    // tech LEF: explicit flag, else resolved from the PDK adapter.
    let tech_lef = match (&cli.tech_lef, &cli.pdk) {
        (Some(t), _) => t.clone(),
        (None, Some(p)) => vyges_layout::pdk::resolve(p, "tech_lef", None)?,
        (None, None) => return Err("need --pdk NAME or --tech-lef PATH".into()),
    };
    // cache path: alongside the PDK's `extract_rules` collateral (the vyges-additions/
    // dir). Use a RAW resolve (the file itself need not exist — e.g. a PDK with no LVS
    // ruleset yet — we only want the directory).
    let cache = match &cli.pdk {
        // A collateral entry may be a URL rather than a local path — the catalog hosts the
        // vyges-additions/ rulesets remotely, so `resolve <pdk> extract_rules` succeeds with an
        // `https://` string for every PDK. Joining a filename onto that and writing it produced a
        // literal `./https:/raw.githubusercontent.com/...` tree in the working directory, with the
        // cached rules landing where nothing would look for them. Only a local absolute path can
        // serve as a cache directory; anything else falls back to caching beside the tech LEF.
        Some(p) => match pdk_store_raw(p, "extract_rules").filter(|er| is_local_path(er)) {
            Some(er) => {
                let dir = std::path::Path::new(&er)
                    .parent()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("{dir}/vyges-extract-rc.rules")
            }
            None => format!("{tech_lef}.vyges-extract-rc.rules"),
        },
        None => format!("{tech_lef}.vyges-extract-rc.rules"),
    };
    if std::path::Path::new(&cache).exists() && !cli.refresh {
        // Re-check the cached deck before handing it back. This is the path that made the
        // original defect invisible: the file already exists, nothing is re-derived, and
        // without this the geometry-only warning is printed exactly once ever — on the run
        // that generated it, quite possibly on someone else's machine.
        if let Ok(r) = vyges_extract::rules::RcRules::load(&cache) {
            warn_if_incomplete(&r);
        }
        return Ok(cache);
    }
    let lef = vyges_loom::lef::Lef::load(&tech_lef).map_err(|e| format!("{tech_lef}: {e}"))?;
    let mut rules = vyges_extract::rules::RcRules::from_lef(&lef);
    // Not every tech LEF carries R/C (some are geometry-only; RC lives in an OpenRCX
    // captable). Fill any missing R/C from a captable — explicit `--captable`, else the
    // PDK's `captable` collateral — mapping the captable's `Metal N` via the LEF stack.
    let captable = cli.captable.clone().or_else(|| {
        cli.pdk
            .as_deref()
            .and_then(|p| vyges_layout::pdk::resolve(p, "captable", None).ok())
    });
    let incomplete = |r: &vyges_extract::rules::RcRules| {
        r.layers
            .values()
            .any(|l| l.res_per_um == 0.0 || l.cap_per_um == 0.0)
    };
    if incomplete(&rules) {
        if let Some(cap) = &captable {
            if let Ok(rcx) = std::fs::read_to_string(cap) {
                let cr = vyges_extract::rules::RcRules::from_captable(&lef.routing_order, &rcx);
                for (n, cl) in &cr.layers {
                    let e = rules.layers.entry(n.clone()).or_insert(*cl);
                    if e.res_per_um == 0.0 {
                        e.res_per_um = cl.res_per_um;
                    }
                    if e.cap_per_um == 0.0 {
                        e.cap_per_um = cl.cap_per_um;
                    }
                }
                for (n, r) in &cr.rsheet {
                    rules.rsheet.entry(n.clone()).or_insert(*r);
                }
                if rules.via_res == 0.0 {
                    rules.via_res = cr.via_res;
                }
            }
        }
    }
    // Warn if R/C are still missing (no captable, or it didn't cover a layer). Note this is
    // the *generation-time* warning only; the same check runs again on every load, because a
    // cached ruleset is reused silently and this message would otherwise be printed once and
    // never again.
    warn_if_incomplete(&rules);
    if let Some(dir) = std::path::Path::new(&cache).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&cache, rules.to_deck()).map_err(|e| format!("{cache}: {e}"))?;
    Ok(cache)
}

fn parse_cli(args: &[String]) -> Cli {
    let mut c = Cli::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                c.out = args.get(i + 1).cloned();
                i += 1;
            }
            "-j" | "--threads" => {
                c.threads = args.get(i + 1).and_then(|s| s.parse().ok());
                i += 1;
            }
            "--pdk" => {
                c.pdk = args.get(i + 1).cloned();
                i += 1;
            }
            "--tech-lef" => {
                c.tech_lef = args.get(i + 1).cloned();
                i += 1;
            }
            "--captable" => {
                c.captable = args.get(i + 1).cloned();
                i += 1;
            }
            "--refresh" => c.refresh = true,
            "--allow-incomplete-rc" => c.allow_incomplete_rc = true,
            "--gds" => {
                c.gds = args.get(i + 1).cloned();
                i += 1;
            }
            "--top" => {
                c.top = args.get(i + 1).cloned();
                i += 1;
            }
            "--layermap" => {
                c.layermap = args.get(i + 1).cloned();
                i += 1;
            }
            "--routing-only" => c.routing_only = true,
            "--geom-out" => {
                c.geom_out = args.get(i + 1).cloned();
                i += 1;
            }
            "--from-dump" => {
                c.from_dump = args.get(i + 1).cloned();
                i += 1;
            }
            "--self-test" => c.self_test = true,
            "--python" => {
                c.python = args.get(i + 1).cloned();
                i += 1;
            }
            "--runner" => {
                c.runner = args.get(i + 1).cloned();
                i += 1;
            }
            "--image" => {
                c.image = args.get(i + 1).cloned();
                i += 1;
            }
            "--mount" => {
                c.mount = args.get(i + 1).cloned();
                i += 1;
            }
            "--driver" => {
                c.driver = args.get(i + 1).cloned();
                i += 1;
            }
            "--date" => {
                c.date = args.get(i + 1).cloned();
                i += 1;
            }
            "--def" => {
                c.def = args.get(i + 1).cloned();
                i += 1;
            }
            "--cell-lef" => {
                c.cell_lef = args.get(i + 1).cloned();
                i += 1;
            }
            "--lib" => {
                c.lib = args.get(i + 1).cloned();
                i += 1;
            }
            "--json" => c.json = true,
            "-q" | "--quiet" => c.quiet = true,
            "-v" | "--verbose" => c.verbose = true,
            "-h" | "--help" => c.help = true,
            "-V" | "--version" => c.version = true,
            "--bug-report" => c.bug_report = true,
            "--feature-request" => c.feature_request = true,
            "--sponsor" => c.sponsor = true,
            "--star" => c.star = true,
            other => c.positionals.push(other.to_string()),
        }
        i += 1;
    }
    c
}

fn write_out(text: &str, cli: &Cli) {
    match &cli.out {
        Some(path) => match std::fs::write(path, text) {
            Ok(_) => {
                if !cli.quiet {
                    println!("wrote {path}");
                }
            }
            Err(e) => {
                eprintln!("error: {path}: {e}");
                exit(1);
            }
        },
        None => print!("{text}"),
    }
}

/// Insert a `coverage` block at the head of a JSON object payload.
fn splice_coverage(json: &str, note: &str) -> String {
    let Some(rest) = json.trim_start().strip_prefix('{') else {
        return json.to_string();
    };
    let esc = note.replace('\\', "\\\\").replace('"', "\\\"");
    let sep = if rest.trim_start().starts_with('}') {
        ""
    } else {
        ","
    };
    format!("{{\"coverage\":{{\"complete\":false,\"note\":\"{esc}\"}}{sep}{rest}")
}

fn render(
    design: &str,
    nets: &[NetParasitics],
    trees: &[Option<RcNetwork>],
    couplings: &[CouplingCap],
    resolver: Option<&vyges_extract::hookup::PinResolver>,
    cli: &Cli,
) -> String {
    if cli.json {
        // Splice the coverage caveat (#72) into the summary when the RC deck this run used
        // was incomplete. The extraction establishes no pass/fail claim, so this changes no
        // verdict -- but a downstream timer consuming these parasitics has no other way to
        // learn they are understated, and silently handing it confident-looking zeros is the
        // failure this whole area keeps producing.
        let j = spef::render_json(design, nets, couplings);
        match &cli.incomplete_rc_note {
            Some(note) => splice_coverage(&j, note),
            None => j,
        }
    } else {
        spef::render_distributed(
            design,
            &Units::default(),
            None,
            nets,
            trees,
            couplings,
            resolver,
        )
    }
}

/// Build the std-cell pin resolver for `run`, opting in only when a cell LEF or a
/// liberty was supplied. Cell-LEF source: `--cell-lef`, else the job's tech LEF
/// (harmless if it carries no MACROs). Directions come from the LEF then liberty;
/// per-load Cin from liberty. A bad collateral path is fatal (fail on wrong file).
fn build_run_resolver(job: &ExtractJob, cli: &Cli) -> Option<vyges_extract::hookup::PinResolver> {
    if cli.cell_lef.is_none() && cli.lib.is_none() {
        return None;
    }
    if job.def.is_empty() {
        return None;
    }
    let def = match vyges_extract::def::Def::load(&job.resolve(&job.def)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {}: {e}", job.def);
            exit(1);
        }
    };
    // cell-LEF source: --cell-lef (as given, relative to CWD), else the job's tech
    // LEF resolved against the job dir. liberty: --lib (relative to CWD).
    let cell_lef = cli
        .cell_lef
        .clone()
        .or_else(|| job.lef.as_deref().map(|p| job.resolve(p)));
    match vyges_extract::hookup::PinResolver::new(&def, cell_lef.as_deref(), cli.lib.as_deref()) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("error: {e}");
            exit(1);
        }
    }
}

fn demo_nets() -> Vec<NetParasitics> {
    vec![
        NetParasitics {
            name: "clk".into(),
            pins: vec![("clkbuf".into(), "X".into()), ("ff0".into(), "CLK".into())],
            res_ohm: 12.5,
            cap_ff: 6.4,
        },
        NetParasitics {
            name: "n0".into(),
            pins: vec![("u0".into(), "Y".into()), ("u1".into(), "A".into())],
            res_ohm: 3.1,
            cap_ff: 1.8,
        },
    ]
}

fn demo_couplings() -> Vec<CouplingCap> {
    vec![CouplingCap {
        a: "clk".into(),
        b: "n0".into(),
        cap_ff: 0.42,
    }]
}

/// Emit the vyges-events causal trail for an extraction result on STDERR (never
/// stdout — that carries the SPEF / JSON report). Extraction produces data, not
/// violations, so the headline is a completion summary (EXTRACT-DONE); any net
/// the extractor could not resolve to a real connection (fewer than two pins) is
/// surfaced as an EXTRACT-UNCONNECTED warning so downstream tooling can react.
fn emit_extract_events(nets: &[NetParasitics], couplings: &[CouplingCap]) {
    use vyges_events::{Event, Severity};
    let e = |sev, code: &str, msg: String, objs: Vec<String>| {
        vyges_events::emit(
            &Event::new("vyges-extract", sev, msg)
                .with_code(code)
                .with_objects(objs),
        );
    };
    for n in nets {
        if n.pins.len() < 2 {
            e(
                Severity::Warn,
                "EXTRACT-UNCONNECTED",
                format!(
                    "net '{}' has {} pin(s) — no complete connection to extract",
                    n.name,
                    n.pins.len()
                ),
                vec![format!("net:{}", n.name)],
            );
        }
    }
    let total_cap_ff: f64 = nets.iter().map(|n| n.cap_ff).sum::<f64>()
        + couplings.iter().map(|c| c.cap_ff).sum::<f64>();
    e(
        Severity::Info,
        "EXTRACT-DONE",
        format!(
            "extracted {} net(s), {} coupling pair(s) ({:.2} fF total capacitance)",
            nets.len(),
            couplings.len(),
            total_cap_ff
        ),
        vec![],
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--describe") {
        // Machine-readable description of `run` for tooling that drives it.
        const DESCRIBE: &str = r#"{
  "schema": "vyges-tool-descriptor/1.1",
  "name": "extract",
  "summary": "foundry-correlated RC parasitic extraction (DEF -> SPEF)",
  "maturity": "workflow-validated",
  "provenance_limitations": [
      "The job names the DEF/LEF and the rules or captable; input_hash covers the job path and arguments, not their contents."
  ],
  "invocation": {
    "args_template": ["run", "{job}"],
    "optional": [ { "arg": "out", "flag": "-o" } ],
    "emits_json": true
  },
  "inputs": {
    "type": "object",
    "required": ["job"],
    "properties": {
      "job": { "type": "string", "description": "path to the extract job file (design, def, rules, corner, temp)" },
      "out": { "type": "string", "description": "write the SPEF to FILE instead of stdout" }
    }
  },
  "artifacts": [ { "role": "spef", "from_arg": "out" } ],
  "assertion": {
    "id": "parasitic-extract",
    "not_applicable": true
  },
  "consumes": ["def", "gds"]
}
"#;
        print!("{DESCRIBE}");
        return;
    }

    let mut cli = parse_cli(&args);

    // Size the rayon thread pool once, before any parallel extraction. Default (flag absent)
    // lets rayon use all available cores; `-j N` caps it (`-j 1` = serial).
    if let Some(n) = cli.threads {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global();
    }

    if cli.bug_report {
        return link("Report a bug (central — vyges/community)", BUG_URL);
    }
    if cli.feature_request {
        return link("Request a feature (central — vyges/community)", FEATURE_URL);
    }
    if cli.sponsor {
        return link("Sponsor Vyges", SPONSOR_URL);
    }
    if cli.star {
        return link("Star vyges-extract on GitHub ⭐", STAR_URL);
    }
    if cli.version {
        println!(
            "vyges-extract {} ({})",
            vyges_extract::VERSION,
            env!("VYGES_GIT_SHA")
        );
        println!("{}", vyges_extract::COPYRIGHT);
        return;
    }
    let cmd = cli.positionals.first().cloned().unwrap_or_default();
    if cli.help || cmd.is_empty() {
        print!("{USAGE}");
        exit(if cmd.is_empty() && !cli.help { 2 } else { 0 });
    }

    match cmd.as_str() {
        "demo" => {
            let (nets, couplings) = (demo_nets(), demo_couplings());
            emit_extract_events(&nets, &couplings);
            write_out(
                &render("vyges_extract_demo", &nets, &[], &couplings, None, &cli),
                &cli,
            )
        }
        "check" => {
            let Some(path) = cli.positionals.get(1) else {
                eprintln!("usage: vyges-extract check JOB");
                exit(2);
            };
            match ExtractJob::load(path) {
                Ok(j) => println!(
                    "OK  design={} def={} rules={} corner={} temp={}",
                    j.design, j.def, j.rules, j.corner, j.temp
                ),
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(2);
                }
            }
        }
        "gen-rc" => {
            // derive (+cache) the per-layer RC ruleset from a PDK tech LEF, and print
            // it — no design/DEF needed. The metal stack is discovered from the LEF.
            match ensure_rc_rules(&cli) {
                Ok(p) => {
                    if !cli.quiet {
                        eprintln!("wrote {p}");
                    }
                    if let Ok(txt) = std::fs::read_to_string(&p) {
                        print!("{txt}");
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(2);
                }
            }
        }
        "run" => {
            let Some(path) = cli.positionals.get(1) else {
                eprintln!("usage: vyges-extract run JOB [-o OUT]");
                exit(2);
            };
            let mut job = match ExtractJob::load(path) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(2);
                }
            };
            // RC rules: explicit job `rules:`, else derived (+cached) from the PDK
            // tech LEF via --pdk / --tech-lef — the metal stack is discovered, not
            // hand-listed.
            if cli.pdk.is_some() || cli.tech_lef.is_some() {
                match ensure_rc_rules(&cli) {
                    Ok(p) => {
                        if cli.verbose {
                            eprintln!("RC rules (from tech LEF): {p}");
                        }
                        job.rules = p;
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        exit(2);
                    }
                }
            } else if job.rules.is_empty() {
                eprintln!("error: job has no `rules:` — pass --pdk NAME or --tech-lef PATH to derive them");
                exit(2);
            }
            // Refuse to extract against a ruleset that says wires are perfect conductors.
            //
            // This check runs on the rules actually about to be used -- whether freshly
            // derived, loaded from the cache, or named by the job's own `rules:` -- because
            // the dangerous case is the silent one: the cache already exists, nothing is
            // re-derived, no warning is printed, and the SPEF comes out full of zeros looking
            // like a clean result. Zero parasitics are not a measurement, they are a missing
            // input, and a tool that cannot tell the difference should say so rather than
            // hand back confident numbers.
            // `if let`, not a `match`: the error arm is deliberately empty, and writing it out
            // invites the reader to look for a second case that is not there.
            if let Ok(r) = vyges_extract::rules::RcRules::load(&job.rules) {
                let (no_r, no_c) = r.incomplete();
                if !no_r.is_empty() || !no_c.is_empty() {
                    // Record it whether or not the run proceeds. If it does proceed (via
                    // the override), the resulting parasitics are understated and the
                    // caveat has to travel with them -- a downstream timer has no other
                    // way to learn it (#72).
                    cli.incomplete_rc_note = Some(format!(
                        "RC deck {} has {} layer(s) with no resistance and {} with no \
                         capacitance; extracted parasitics are understated",
                        job.rules,
                        no_r.len(),
                        no_c.len()
                    ));
                }
                if warn_if_incomplete(&r) && !cli.allow_incomplete_rc {
                    eprintln!(
                        "error: {} has layers with no R and/or no C, so extraction would \
                         report parasitics that are partly or wholly zero.\n       Supply \
                         the missing numbers with --captable <rcx_rules>, or pass \
                         --allow-incomplete-rc to extract anyway and accept that the \
                         result understates parasitics.",
                        job.rules
                    );
                    exit(2);
                }
            }
            // An unreadable ruleset is not this check's job to diagnose -- extraction is about to
            // open the same file and will report it with better context.
            match engine::extract(&job) {
                Ok(ex) => {
                    if cli.verbose {
                        eprintln!(
                            "extracted {} net(s), {} coupling pair(s) from {}",
                            ex.nets.len(),
                            ex.couplings.len(),
                            job.def
                        );
                    }
                    emit_extract_events(&ex.nets, &ex.couplings);
                    // Std-cell pin hookup: mark *CONN driver/load + per-load Cin from
                    // the DEF placement + cell LEF (--cell-lef, else the job's tech LEF
                    // if it carries MACROs) + liberty (--lib). Skipped when neither a
                    // cell LEF nor a liberty is available.
                    let resolver = build_run_resolver(&job, &cli);
                    if cli.verbose {
                        if let Some(r) = &resolver {
                            if r.active() {
                                eprintln!(
                                    "std-cell hookup: *CONN direction + Cin from DEF/LEF/liberty"
                                );
                            }
                        }
                    }
                    let res_ref = resolver.as_ref().filter(|r| r.active());
                    write_out(
                        &render(
                            &job.design,
                            &ex.nets,
                            &ex.trees,
                            &ex.couplings,
                            res_ref,
                            &cli,
                        ),
                        &cli,
                    );
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(1);
                }
            }
        }
        "klayout2spef" => klayout2spef(&cli),
        other => {
            eprintln!("vyges-extract: unknown command {other:?}\n");
            print!("{USAGE}");
            exit(2);
        }
    }
}

/// `klayout2spef` — extract SPEF (+ EM geom sidecar) from a GDS via headless
/// KLayout. Runs the driver (or `--from-dump` / `--self-test` offline), then
/// writes SPEF and the sidecar and emits the vyges-events trail.
fn klayout2spef(cli: &Cli) {
    use vyges_extract::klayout::{self as klf, KlOpts};

    if cli.self_test {
        match klf::self_test() {
            Ok(s) => {
                println!("{s}");
                return;
            }
            Err(e) => {
                eprintln!("self-test FAILED: {e}");
                exit(1);
            }
        }
    }

    // Build the python invocation. A container runner is just a prefix that mounts
    // a directory into the image and runs its `python3` (KLayout pymod on PATH).
    let python: Option<String> = if let Some(runner) = &cli.runner {
        let image = cli
            .image
            .clone()
            .unwrap_or_else(|| "ghcr.io/vyges-tools/vyges-klayout:0.30.9".to_string());
        let mount = cli.mount.clone().unwrap_or_else(|| {
            // default: the GDS's directory (absolute), where the driver is staged
            cli.gds
                .as_deref()
                .and_then(|g| std::path::Path::new(g).parent())
                .and_then(|p| std::fs::canonicalize(p).ok())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        Some(format!(
            "{runner} run --rm --userns=keep-id -v {mount}:{mount} -w {mount} {image} python3"
        ))
    } else {
        cli.python.clone()
    };

    let dump = match &cli.from_dump {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("error: {p}: {e}");
                exit(2);
            }
        },
        None => {
            // driving KLayout: gds + layermap are required
            if cli.gds.is_none() || cli.layermap.is_none() {
                eprintln!("usage: vyges-extract klayout2spef --gds FILE --layermap FILE [--top CELL] [-o OUT]\n       (or --from-dump FILE, or --self-test)");
                exit(2);
            }
            None
        }
    };

    let design = cli
        .top
        .clone()
        .or_else(|| {
            cli.gds.as_deref().map(|g| {
                std::path::Path::new(g)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "top".into())
            })
        })
        .unwrap_or_else(|| "top".into());

    let opts = KlOpts {
        gds: cli.gds.clone().unwrap_or_default(),
        top: cli.top.clone().unwrap_or_default(),
        layermap: cli.layermap.clone().unwrap_or_default(),
        design,
        routing_only: cli.routing_only,
        python: python
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default(),
        driver: cli.driver.clone(),
        version: vyges_extract::VERSION.to_string(),
        date: cli.date.clone(),
        def: cli.def.clone(),
        cell_lef: cli.cell_lef.clone(),
        lib: cli.lib.clone(),
    };

    match klf::run(&opts, dump) {
        Ok(res) => {
            emit_klayout_events(&res);
            write_out(&res.spef, cli);
            // geom sidecar: --geom-out, else alongside -o (or stderr note for stdout)
            let geom_path = cli.geom_out.clone().or_else(|| {
                cli.out.as_ref().map(|o| {
                    let p = std::path::Path::new(o);
                    let stem = p
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "out".into());
                    p.with_file_name(format!("{stem}.emgeom"))
                        .to_string_lossy()
                        .into_owned()
                })
            });
            match geom_path {
                Some(gp) => match std::fs::write(&gp, &res.geom) {
                    Ok(_) => {
                        if !cli.quiet {
                            println!("wrote {gp}");
                        }
                    }
                    Err(e) => {
                        eprintln!("error: {gp}: {e}");
                        exit(1);
                    }
                },
                None => eprintln!(
                    "note: EM geom sidecar not written (stdout SPEF); pass --geom-out FILE"
                ),
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            exit(1);
        }
    }
}

/// vyges-events trail for a KLayout extraction (STDERR — stdout carries SPEF).
fn emit_klayout_events(res: &vyges_extract::klayout::KlResult) {
    use vyges_events::{Event, Severity};
    if res.n_nets == 0 {
        vyges_events::emit(
            &Event::new(
                "vyges-extract",
                Severity::Warn,
                "KLayout extraction produced 0 nets — check --top / --layermap".to_string(),
            )
            .with_code("KLAYOUT-EXTRACT-EMPTY"),
        );
    }
    vyges_events::emit(
        &Event::new(
            "vyges-extract",
            Severity::Info,
            format!(
                "KLayout→SPEF: {} net(s), {} metal segment(s), {} pin(s) hooked, {:.2} fF total; EM geom sidecar emitted",
                res.n_nets, res.n_segs, res.n_pins, res.total_cap_ff
            ),
        )
        .with_code("KLAYOUT-EXTRACT-DONE"),
    );
}

#[cfg(test)]
mod tests {
    use super::is_local_path;

    /// A collateral entry that is a URL must never be treated as a directory to write into.
    /// The catalog hosts every PDK's vyges-additions/ ruleset remotely, so this is the common
    /// case, not an edge case: before this check, `gen-rc --pdk <any>` created a literal
    /// `./https:/raw.githubusercontent.com/...` tree in the working directory.
    #[test]
    fn a_url_is_not_a_cache_directory() {
        for url in [
            "https://raw.githubusercontent.com/vyges-tools/pdk-catalog/main/descriptors/\
             vyges-additions/asap7/vyges-extract.rules",
            "http://example.invalid/a.rules",
            "s3://bucket/key.rules",
            "git+ssh://host/repo.git",
        ] {
            assert!(!is_local_path(url), "{url} must not be used as a path");
        }
    }

    /// A relative path is rejected too: the directory it names depends on the working directory
    /// the tool happened to be invoked from, which is not a property of the PDK.
    #[test]
    fn only_an_absolute_local_path_is_a_cache_directory() {
        assert!(is_local_path(
            "/opt/pdk/vyges-additions/asap7/vyges-extract.rules"
        ));
        assert!(!is_local_path("vyges-additions/asap7/vyges-extract.rules"));
        assert!(!is_local_path("./rules"));
    }
}
