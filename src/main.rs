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
  vyges-extract run   JOB [-o OUT] [--json]
  vyges-extract check JOB
  vyges-extract demo  [-o OUT] [--json]

flags:
  -o FILE          write output to FILE (default: stdout)
  --json           per-net parasitics summary as JSON instead of SPEF
  -q, --quiet      suppress non-essential output
  -v, --verbose    extra detail on stderr
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
    json: bool,
    quiet: bool,
    verbose: bool,
    help: bool,
    version: bool,
    bug_report: bool,
    feature_request: bool,
    sponsor: bool,
    star: bool,
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
    let cli = parse_cli(&args);

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
        "run" => {
            let Some(path) = cli.positionals.get(1) else {
                eprintln!("usage: vyges-extract run JOB [-o OUT]");
                exit(2);
            };
            let job = match ExtractJob::load(path) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(2);
                }
            };
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
