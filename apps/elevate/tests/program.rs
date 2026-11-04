//! The helper's program, as the desktop app starts it.

#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    reason = "the tests start the helper's program, and fail fast"
)]

use std::process::{Command, Stdio};

use cleanse_engine::{Probe, System};

fn elevated() -> bool {
    System::new().unwrap().is_elevated()
}

/// It serves only as an administrator or root, and only on Linux until it has a pipe of its own
/// on Windows.
#[test]
fn says_why_when_it_cant_serve() {
    let output = Command::new(env!("CARGO_BIN_EXE_cleanse-elevate"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !elevated() {
        assert!(!output.status.success());
        assert!(stderr.contains("administrator or root"), "{stderr}");
    } else if cfg!(target_os = "linux") {
        // Its input ends at once, as when the window closes.
        assert!(output.status.success(), "{stderr}");
    } else {
        assert!(!output.status.success());
        assert!(
            stderr.contains("only talk to the window on Linux"),
            "{stderr}"
        );
    }
}

/// As pkexec starts it, with pipes for its input and output. CI runs the tests named
/// `when_elevated` as root on Linux, in a job of their own.
#[cfg(target_os = "linux")]
#[test]
fn serves_system_cleanup_over_its_input_and_output_when_elevated() {
    use std::io::BufReader;

    use cleanse_elevate::client::Client;
    use cleanse_elevate::protocol::{Request, Response};

    if !elevated() {
        assert!(
            std::env::var_os("CLEANSE_TEST_AS_ROOT").is_none(),
            "CLEANSE_TEST_AS_ROOT is set, so this has to run as root"
        );
        eprintln!("skipped: needs root");
        return;
    }
    let mut helper = Command::new(env!("CARGO_BIN_EXE_cleanse-elevate"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let answers = BufReader::new(helper.stdout.take().unwrap());
    let client = Client::connect(answers, helper.stdin.take().unwrap()).unwrap();

    let results = client.ask(&Request::Scan {}, &mut |_| {}).unwrap();
    let history = client.ask(&Request::History {}, &mut |_| {}).unwrap();

    assert!(matches!(results, Response::Results { .. }), "{results:?}");
    assert!(matches!(history, Response::History { .. }), "{history:?}");
    drop(client);
    assert!(helper.wait().unwrap().success());
}
