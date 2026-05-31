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
";

#[derive(Default)]
struct Cli {
    positionals: Vec<String>,
    out: Option<String>,
    json: bool,
    quiet: bool,
    verbose: bool,
    help: bool,
    version: bool,
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

fn render(design: &str, nets: &[NetParasitics], couplings: &[CouplingCap], cli: &Cli) -> String {
    if cli.json {
        spef::render_json(design, nets, couplings)
    } else {
        spef::render(design, &Units::default(), None, nets, couplings)
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

    if cli.version {
        println!("vyges-extract {}", vyges_extract::VERSION);
        return;
    }
    let cmd = cli.positionals.first().cloned().unwrap_or_default();
    if cli.help || cmd.is_empty() {
        print!("{USAGE}");
        exit(if cmd.is_empty() && !cli.help { 2 } else { 0 });
    }

    match cmd.as_str() {
        "demo" => {
            write_out(&render("vyges_extract_demo", &demo_nets(), &demo_couplings(), &cli), &cli)
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
                    write_out(&render(&job.design, &ex.nets, &ex.couplings, &cli), &cli);
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
