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
  desktop                           run the desktop app's checks: clippy, tests, bindings, Biome, types, frontend tests and build
  gen-tree <dir> [--entries <n>]    create a synthetic directory tree for benchmarks (default 200000 entries)
  help                              show this message";

struct Check {
    name: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    install: Option<&'static str>,
}

/// On Windows, pnpm is a batch file, which has to be named in full.
const PNPM: &str = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };

/// Where the desktop app's TypeScript bindings are, which its Rust tests write.
const BINDINGS: &str = "apps/desktop/src/bindings";

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
            "--exclude",
            "cleanse-desktop",
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
        args: &[
            "nextest",
            "run",
            "--workspace",
            "--exclude",
            "cleanse-desktop",
            "--locked",
        ],
        install: Some("cargo install --locked cargo-nextest"),
    },
    Check {
        name: "doc tests",
        program: "cargo",
        args: &[
            "test",
            "--workspace",
            "--exclude",
            "cleanse-desktop",
            "--doc",
            "--locked",
        ],
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

/// The desktop app's Rust checks. Its views' tests also write the TypeScript bindings.
const DESKTOP_RUST_CHECKS: &[Check] = &[
    Check {
        name: "clippy",
        program: "cargo",
        args: &[
            "clippy",
            "--package",
            "cleanse-desktop",
            "--package",
            "cleanse-desktop-views",
            "--all-targets",
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
        args: &[
            "nextest",
            "run",
            "--package",
            "cleanse-desktop",
            "--package",
            "cleanse-desktop-views",
            "--locked",
        ],
        install: Some("cargo install --locked cargo-nextest"),
    },
];

/// The desktop app's frontend checks, which read the bindings the Rust tests wrote.
const DESKTOP_FRONTEND_CHECKS: &[Check] = &[
    Check {
        name: "pnpm install",
        program: PNPM,
        args: &["--dir", "apps/desktop", "install", "--frozen-lockfile"],
        install: Some("corepack enable"),
    },
    Check {
        name: "Biome",
        program: PNPM,
        args: &["--dir", "apps/desktop", "run", "check"],
        install: None,
    },
    Check {
        name: "types",
        program: PNPM,
        args: &["--dir", "apps/desktop", "run", "typecheck"],
        install: None,
    },
    Check {
        name: "frontend tests",
        program: PNPM,
        args: &["--dir", "apps/desktop", "run", "test"],
        install: None,
    },
    Check {
        name: "frontend build",
        program: PNPM,
        args: &["--dir", "apps/desktop", "run", "build"],
        install: None,
    },
];

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("desktop") => desktop(),
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

fn ci() -> ExitCode {
    report(&run_checks(CI_CHECKS))
}

fn desktop() -> ExitCode {
    let mut failed = run_checks(DESKTOP_RUST_CHECKS);
    eprintln!("\n==> bindings");
    if !bindings_current() {
        failed.push("bindings");
    }
    failed.extend(run_checks(DESKTOP_FRONTEND_CHECKS));
    report(&failed)
}

/// Runs every check, even after a failure, so one run reports everything that needs fixing,
/// and returns the names of those that failed.
fn run_checks(checks: &[Check]) -> Vec<&'static str> {
    let mut failed = Vec::new();
    for check in checks {
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
    failed
}

/// Whether the bindings the tests just wrote are the ones in Git's index, so none changed or
/// appeared that aren't staged.
fn bindings_current() -> bool {
    match Command::new("git")
        .args(["status", "--porcelain", "--", BINDINGS])
        .output()
    {
        Ok(output) if output.status.success() => {
            // Each line is two status letters and a path; the second is the working tree's.
            let stdout = String::from_utf8_lossy(&output.stdout);
            let changed: Vec<&str> = stdout
                .lines()
                .filter(|line| line.as_bytes().get(1) != Some(&b' '))
                .collect();
            if changed.is_empty() {
                return true;
            }
            eprintln!(
                "{BINDINGS} changed when the tests ran. Commit the new bindings:\n{}",
                changed.join("\n")
            );
            false
        }
        Ok(output) => {
            eprintln!(
                "could not check {BINDINGS}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            false
        }
        Err(err) => {
            eprintln!("could not run `git`: {err}");
            false
        }
    }
}

fn report(failed: &[&str]) -> ExitCode {
    if failed.is_empty() {
        eprintln!("\nall checks passed");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nfailed: {}", failed.join(", "));
        ExitCode::FAILURE
    }
}
