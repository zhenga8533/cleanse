//! Repository automation, run as `cargo xtask <task>`.

#![allow(
    clippy::disallowed_methods,
    reason = "xtask runs development tools and writes benchmark fixtures"
)]

mod gen_tree;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <task>

tasks:
  ci                                run the same checks as CI: rustfmt, clippy, tests, crates.io packages, cargo-deny, typos, taplo
  desktop                           run the desktop app's checks: clippy, tests, bindings, Biome, types, frontend tests and build
  gen-tree <dir> [--entries <n>]    create a synthetic directory tree for benchmarks (default 200000 entries)
  helper                            build the desktop app's elevated helper for its installers, in apps/desktop/src-tauri/binaries
  packages [--allow-dirty]          package the crates the release publishes to crates.io, and build each from its package
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

/// The packages `cargo publish --workspace` publishes: every one without `publish = false`.
const PUBLISHED: &[&str] = &[
    "cleanse-model",
    "cleanse-platform",
    "cleanse-core",
    "cleanse-exec",
    "cleanse-engine",
    "cleanse-cli",
];

/// Where the packages check builds, apart from everything else, so cleaning it there rebuilds
/// nothing else.
const PACKAGE_CHECK_TARGET: &str = "target/package-check";

/// Where the desktop app's installers take its elevated helper from, as
/// `apps/desktop/src-tauri/installer/` says.
const STAGED_HELPER: &str = "apps/desktop/src-tauri/binaries";

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
        Some("helper") => helper(),
        Some("packages") => match &args[1..] {
            [] => report(&packages(false)),
            [flag] if flag == "--allow-dirty" => report(&packages(true)),
            _ => {
                eprintln!("usage: cargo xtask packages [--allow-dirty]");
                ExitCode::from(2)
            }
        },
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
    let mut failed = run_checks(CI_CHECKS);
    // Unlike CI, which checks out a clean tree, this runs before changes are committed, which
    // `cargo publish` refuses without `--allow-dirty`.
    failed.extend(packages(true));
    report(&failed)
}

/// Packages the crates crates.io gets, as the release does, and builds each from its package
/// alone, so a file one needs but doesn't include fails here rather than at release.
///
/// That build takes the other crates from their packages too, as from a registry, and cargo
/// never rebuilds a registry crate at a version it has already built. So after a change at the
/// same version, it would build against the old code. Cleaning the crates' builds first keeps
/// it to the code being packaged.
fn packages(allow_dirty: bool) -> Vec<&'static str> {
    const NAME: &str = "crates.io packages";
    eprintln!("\n==> {NAME}");
    let mut clean = Command::new("cargo");
    clean.args(["clean", "--target-dir", PACKAGE_CHECK_TARGET]);
    for package in PUBLISHED {
        clean.args(["--package", package]);
    }
    let mut publish = Command::new("cargo");
    publish.args([
        "publish",
        "--workspace",
        "--dry-run",
        "--locked",
        "--target-dir",
        PACKAGE_CHECK_TARGET,
    ]);
    if allow_dirty {
        publish.arg("--allow-dirty");
    }
    if succeeds(&mut clean) && succeeds(&mut publish) {
        Vec::new()
    } else {
        vec![NAME]
    }
}

/// Runs `command`, saying why if it couldn't.
fn succeeds(command: &mut Command) -> bool {
    match command.status() {
        Ok(status) => status.success(),
        Err(err) => {
            eprintln!("could not run `{}`: {err}", command.get_program().display());
            false
        }
    }
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

/// Builds the elevated helper for release, and puts it where the installers take it from, named
/// as Tauri's `externalBin` wants: with the target it's built for, such as
/// `cleanse-elevate-x86_64-pc-windows-msvc.exe`.
fn helper() -> ExitCode {
    let host = match host_target() {
        Ok(host) => host,
        Err(err) => {
            eprintln!("could not tell what this computer builds for: {err}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("==> building cleanse-elevate");
    match Command::new("cargo")
        .args([
            "build",
            "--release",
            "--locked",
            "--package",
            "cleanse-elevate",
        ])
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(_) => return ExitCode::FAILURE,
        Err(err) => {
            eprintln!("could not run `cargo`: {err}");
            return ExitCode::FAILURE;
        }
    }
    let exe = env::consts::EXE_SUFFIX;
    let target =
        env::var_os("CARGO_TARGET_DIR").map_or_else(|| PathBuf::from("target"), PathBuf::from);
    let built = target.join("release").join(format!("cleanse-elevate{exe}"));
    let staged = Path::new(STAGED_HELPER).join(format!("cleanse-elevate-{host}{exe}"));
    if let Err(err) = fs::create_dir_all(STAGED_HELPER).and_then(|()| fs::copy(&built, &staged)) {
        eprintln!(
            "could not copy {} to {}: {err}",
            built.display(),
            staged.display()
        );
        return ExitCode::FAILURE;
    }
    eprintln!("staged {}", staged.display());
    ExitCode::SUCCESS
}

/// The target this computer's Rust toolchain builds for by default, as `rustc -vV` says.
fn host_target() -> Result<String, String> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|err| format!("could not run `rustc`: {err}"))?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| "`rustc -vV` didn't say".to_owned())
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
