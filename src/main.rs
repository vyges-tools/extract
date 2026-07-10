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
  vyges-extract gen-rc (--pdk NAME | --tech-lef PATH) [--refresh]
  vyges-extract check  JOB
  vyges-extract demo   [-o OUT] [--json]

RC rules come from a job's `rules:`, or are DERIVED from the PDK tech LEF via
--pdk / --tech-lef (the metal stack is discovered, not hand-listed) and cached
as vyges-additions/<pdk>/vyges-extract-rc.rules (regenerate with --refresh). When
a tech LEF is geometry-only (no R/C), an OpenRCX captable supplies the numbers.

flags:
  --pdk NAME       derive RC rules from the PDK tech LEF (resolved via pdk-store)
  --tech-lef PATH  derive RC rules from this tech LEF directly
  --captable PATH  OpenRCX rules file for R/C the LEF lacks (else pdk-store captable)
  --refresh        re-derive the cached RC rules
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
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
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
    let out = std::process::Command::new(prog).args(["resolve", pdk, key]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
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
        Some(p) => match pdk_store_raw(p, "extract_rules") {
            Some(er) => {
                let dir = std::path::Path::new(&er).parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                format!("{dir}/vyges-extract-rc.rules")
            }
            None => format!("{tech_lef}.vyges-extract-rc.rules"),
        },
        None => format!("{tech_lef}.vyges-extract-rc.rules"),
    };
    if std::path::Path::new(&cache).exists() && !cli.refresh {
        return Ok(cache);
    }
    let lef = vyges_loom::lef::Lef::load(&tech_lef).map_err(|e| format!("{tech_lef}: {e}"))?;
    let mut rules = vyges_extract::rules::RcRules::from_lef(&lef);
    // Not every tech LEF carries R/C (some are geometry-only; RC lives in an OpenRCX
    // captable). Fill any missing R/C from a captable — explicit `--captable`, else the
    // PDK's `captable` collateral — mapping the captable's `Metal N` via the LEF stack.
    let captable = cli
        .captable
        .clone()
        .or_else(|| cli.pdk.as_deref().and_then(|p| vyges_layout::pdk::resolve(p, "captable", None).ok()));
    let incomplete = |r: &vyges_extract::rules::RcRules| r.layers.values().any(|l| l.res_per_um == 0.0 || l.cap_per_um == 0.0);
    if incomplete(&rules) {
        if let Some(cap) = &captable {
            if let Ok(rcx) = std::fs::read_to_string(cap) {
                let cr = vyges_extract::rules::RcRules::from_captable(&lef.routing_order, &rcx);
                for (n, cl) in &cr.layers {
                    let e = rules.layers.entry(n.clone()).or_insert_with(|| cl.clone());
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
    // Warn if R/C are still missing (no captable, or it didn't cover a layer).
    let no_r: Vec<_> = rules.layers.iter().filter(|(_, l)| l.res_per_um == 0.0).map(|(n, _)| n.as_str()).collect();
    let no_c: Vec<_> = rules.layers.iter().filter(|(_, l)| l.cap_per_um == 0.0).map(|(n, _)| n.as_str()).collect();
    if !no_r.is_empty() {
        eprintln!("warning: no resistance for [{}] — tech LEF is geometry-only; pass --captable <rcx_rules>", no_r.join(", "));
    } else if !no_c.is_empty() {
        eprintln!("warning: no capacitance for [{}] — RC is resistance-only; pass --captable for cap", no_c.join(", "));
    }
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

fn render(
    design: &str,
    nets: &[NetParasitics],
    trees: &[Option<RcNetwork>],
    couplings: &[CouplingCap],
    cli: &Cli,
) -> String {
    if cli.json {
        spef::render_json(design, nets, couplings)
    } else {
        spef::render_distributed(design, &Units::default(), None, nets, trees, couplings)
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
    vec![CouplingCap { a: "clk".into(), b: "n0".into(), cap_ff: 0.42 }]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--describe") {
        // Machine-readable description of `run` for tooling that drives it.
        const DESCRIBE: &str = r#"{
  "name": "extract",
  "summary": "foundry-correlated RC parasitic extraction (DEF -> SPEF)",
  "invocation": {
    "args_template": ["run", "{job}"],
    "emits_json": true
  },
  "inputs": {
    "type": "object",
    "required": ["job"],
    "properties": {
      "job": { "type": "string", "description": "path to the extract job file (design, def, rules, corner, temp)" }
    }
  },
  "artifacts": [ { "role": "spef" } ],
  "consumes": ["def", "gds"]
}
"#;
        print!("{DESCRIBE}");
        return;
    }

    let cli = parse_cli(&args);

    // Size the rayon thread pool once, before any parallel extraction. Default (flag absent)
    // lets rayon use all available cores; `-j N` caps it (`-j 1` = serial).
    if let Some(n) = cli.threads {
        let _ = rayon::ThreadPoolBuilder::new().num_threads(n).build_global();
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
        println!("vyges-extract {} ({})", vyges_extract::VERSION, env!("VYGES_GIT_SHA"));
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
            write_out(&render("vyges_extract_demo", &demo_nets(), &[], &demo_couplings(), &cli), &cli)
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
                    write_out(&render(&job.design, &ex.nets, &ex.trees, &ex.couplings, &cli), &cli);
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(1);
                }
            }
        }
        other => {
            eprintln!("vyges-extract: unknown command {other:?}\n");
            print!("{USAGE}");
            exit(2);
        }
    }
}
