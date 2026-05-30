//! vyges-extract CLI.
//!
//!   vyges-extract run   JOB [-o OUT.spef]   extract -> SPEF (rule-based, offline)
//!   vyges-extract check JOB                 parse + validate the job, print summary
//!   vyges-extract demo  [-o OUT.spef]       emit a sample .spef (no inputs) to show output

use std::process::exit;

use vyges_extract::engine;
use vyges_extract::job::ExtractJob;
use vyges_extract::rc::NetParasitics;
use vyges_extract::spef::{self, Units};

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn write_out(text: &str, out: Option<String>) {
    match out {
        Some(path) => match std::fs::write(&path, text) {
            Ok(_) => println!("wrote {path}"),
            Err(e) => {
                eprintln!("error: {path}: {e}");
                exit(1);
            }
        },
        None => print!("{text}"),
    }
}

fn demo_spef() -> String {
    let nets = vec![
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
    ];
    spef::render("vyges_extract_demo", &Units::default(), None, &nets)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    match cmd {
        "--version" | "-V" => println!("vyges-extract {}", vyges_extract::VERSION),
        "demo" => write_out(&demo_spef(), arg_after(&args, "-o")),
        "check" => {
            let Some(path) = args.get(1) else {
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
            let Some(path) = args.get(1) else {
                eprintln!("usage: vyges-extract run JOB [-o OUT.spef]");
                exit(2);
            };
            let job = match ExtractJob::load(path) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(2);
                }
            };
            match engine::run_to_spef(&job) {
                Ok(spef) => write_out(&spef, arg_after(&args, "-o")),
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(1);
                }
            }
        }
        _ => {
            eprintln!(
                "vyges-extract {}\nusage: vyges-extract <run|check|demo|--version>",
                vyges_extract::VERSION
            );
            exit(2);
        }
    }
}
