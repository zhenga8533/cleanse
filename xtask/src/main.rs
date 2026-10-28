//! Repository automation, run as `cargo xtask <task>`.

#![allow(
    clippy::disallowed_methods,
    reason = "xtask runs development tools and writes benchmark fixtures"
)]

mod gen_tree;

use std::env;
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <task>

tasks:
  ci                                run the same checks as CI: rustfmt, clippy, tests, cargo-deny, typos, taplo
  gen-tree <dir> [--entries <n>]    create a synthetic directory tree for benchmarks (default 200000 entries)
  help                              show this message";

struct Check {
    name: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    install: Option<&'static str>,
}

const CI_CHECKS: &[Check] = &[
    Check {
        name: "rustfmt",
        program: "cargo",
        args: &["fmt", "--all", "--check"],
        install: None,
    },
    Check {
        name: "clippy",
        program: "cargo",
        args: &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
        install: None,
    },
    Check {
        name: "tests",
        program: "cargo",
        args: &["nextest", "run", "--workspace", "--locked"],
        install: Some("cargo install --locked cargo-nextest"),
    },
    Check {
        name: "doc tests",
        program: "cargo",
        args: &["test", "--workspace", "--doc", "--locked"],
        install: None,
    },
    Check {
        name: "cargo-deny",
        program: "cargo",
        args: &["deny", "check"],
        install: Some("cargo install --locked cargo-deny"),
    },
    Check {
        name: "typos",
        program: "typos",
        args: &[],
        install: Some("cargo install --locked typos-cli"),
    },
    Check {
        name: "taplo",
        program: "taplo",
        args: &["fmt", "--check"],
        install: Some("cargo install --locked taplo-cli"),
    },
];

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("gen-tree") => gen_tree::run(&args[1..]),
        Some("help" | "-h" | "--help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown task `{other}`\n\n{USAGE}");
            ExitCode::from(2)
        }
        None => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Runs every check, even after a failure, so one run reports everything that needs fixing.
fn ci() -> ExitCode {
    let mut failed = Vec::new();

    for check in CI_CHECKS {
        eprintln!("\n==> {}", check.name);
        let passed = match Command::new(check.program).args(check.args).status() {
            Ok(status) => status.success(),
            Err(err) => {
                eprintln!("could not run `{}`: {err}", check.program);
                false
            }
        };
        if !passed {
            if let Some(install) = check.install {
                eprintln!("(if the tool is missing, install it with `{install}`)");
            }
            failed.push(check.name);
        }
    }

    if failed.is_empty() {
        eprintln!("\nall checks passed");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nfailed: {}", failed.join(", "));
        ExitCode::FAILURE
    }
}
