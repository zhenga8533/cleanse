#![allow(
    clippy::disallowed_methods,
    reason = "integration tests spawn the cleanse binary"
)]

use std::process::Command;

fn cleanse() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cleanse"))
}

#[test]
fn version_flag_prints_name_and_version() {
    let output = cleanse().arg("--version").output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        concat!("cleanse ", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn bare_invocation_shows_help_instead_of_doing_anything() {
    let output = cleanse().output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("Usage: cleanse"),
        "unexpected stderr: {stderr}"
    );
}
