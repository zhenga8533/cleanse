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

/// Whether to run a test that needs an administrator or root. CI's Windows runners are
/// administrators, and a CI job of its own runs the tests named `when_elevated` as root on Linux.
#[cfg(any(windows, target_os = "linux"))]
fn elevated_or_skip() -> bool {
    if elevated() {
        return true;
    }
    let required = if cfg!(windows) {
        "CI"
    } else {
        "CLEANSE_TEST_AS_ROOT"
    };
    assert!(
        std::env::var_os(required).is_none(),
        "{required} is set, so this has to run as an administrator or root"
    );
    eprintln!("skipped: needs an administrator or root");
    false
}

/// It serves only as an administrator or root, only on Windows and Linux, and on Windows only
/// when told which pipes to make for which window.
#[test]
fn says_why_when_it_cant_serve() {
    let started = Command::new(env!("CARGO_BIN_EXE_cleanse-elevate"))
        .stdin(Stdio::null())
        .output();
    if cfg!(windows) && !elevated() {
        // Its manifest asks for an administrator, and only the UAC prompt can give it one.
        const ERROR_ELEVATION_REQUIRED: i32 = 740;
        let error = started.unwrap_err();
        assert_eq!(
            error.raw_os_error(),
            Some(ERROR_ELEVATION_REQUIRED),
            "{error}"
        );
        return;
    }
    let output = started.unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    if cfg!(target_os = "macos") {
        assert!(!output.status.success());
        assert!(stderr.contains("only for Windows and Linux"), "{stderr}");
    } else if !elevated() {
        assert!(!output.status.success());
        assert!(stderr.contains("administrator or root"), "{stderr}");
    } else if cfg!(windows) {
        assert!(!output.status.success());
        assert!(stderr.contains("--pipe <name> --parent"), "{stderr}");
    } else {
        // Its input ends at once, as when the window closes.
        assert!(output.status.success(), "{stderr}");
    }
}

/// Windows shows the UAC prompt for it because its manifest asks for an administrator.
#[cfg(windows)]
#[test]
fn asks_windows_for_an_administrator() {
    let program = std::fs::read(env!("CARGO_BIN_EXE_cleanse-elevate")).unwrap();
    let asked = b"<requestedExecutionLevel level='requireAdministrator' />";

    assert!(program.windows(asked.len()).any(|bytes| bytes == asked));
}

/// As pkexec starts it, with pipes for its input and output.
#[cfg(target_os = "linux")]
#[test]
fn serves_system_cleanup_over_its_input_and_output_when_elevated() {
    use std::io::BufReader;

    use cleanse_elevate::client::Client;
    use cleanse_elevate::protocol::{Request, Response};

    if !elevated_or_skip() {
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

/// It serves only the desktop app's program, installed beside it.
#[cfg(windows)]
#[test]
fn refuses_a_window_that_isnt_the_desktop_app_when_elevated() {
    if !elevated_or_skip() {
        return;
    }
    let name = format!("cleanse-test-{}", std::process::id());
    let output = Command::new(env!("CARGO_BIN_EXE_cleanse-elevate"))
        .args(["--pipe", &name, "--parent", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.contains(r"\cleanse-desktop.exe"), "{stderr}");
}

/// As the desktop app starts it on Windows, with the name of the pipes to make and its own
/// process ID. The desktop app is played by a copy of this test program, named as the app is,
/// beside a copy of the helper. It asks for nothing that scans: the server's own tests do, and
/// a scan of the real system folders can take minutes.
#[cfg(windows)]
#[test]
fn serves_the_desktop_app_over_its_pipes_when_elevated() {
    const AS_DESKTOP: &str = "CLEANSE_TEST_AS_DESKTOP";
    const TEST: &str = "serves_the_desktop_app_over_its_pipes_when_elevated";

    if std::env::var_os(AS_DESKTOP).is_some() {
        be_the_desktop_app();
        return;
    }
    if !elevated_or_skip() {
        return;
    }
    // Not the system's temporary folder, whose path may hold 8.3 short names that the program
    // paths Windows reports wouldn't match.
    let folder = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let desktop = folder.path().join("cleanse-desktop.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &desktop).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_cleanse-elevate"),
        folder.path().join("cleanse-elevate.exe"),
    )
    .unwrap();

    let output = Command::new(&desktop)
        .args(["--exact", TEST, "--nocapture"])
        .env(AS_DESKTOP, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "{stdout}{stderr}");
    assert!(stdout.contains("1 passed"), "{stdout}{stderr}");
}

#[cfg(windows)]
/// Starts the helper as the app does, which needs no UAC prompt as an administrator.
#[cfg(windows)]
fn be_the_desktop_app() {
    let program = std::env::current_exe()
        .unwrap()
        .with_file_name("cleanse-elevate.exe");

    ask_quarantine_and_history(&program);
}

/// Starts the helper at `program` as the app does, asks it for what's in quarantine and the
/// history, then closes its requests, and checks that it stopped cleanly.
#[cfg(any(windows, target_os = "linux"))]
fn ask_quarantine_and_history(program: &std::path::Path) {
    use cleanse_elevate::client::Client;
    use cleanse_elevate::protocol::{Request, Response};
    use cleanse_exec::helper::{self, Installed};

    let mut launched = helper::launch(&Installed::unchecked(program.to_path_buf()), None).unwrap();
    let client = Client::connect(launched.answers, launched.requests).unwrap();

    let quarantine = client.ask(&Request::Quarantined {}, &mut |_| {}).unwrap();
    let history = client.ask(&Request::History {}, &mut |_| {}).unwrap();

    assert!(
        matches!(quarantine, Response::Quarantine { .. }),
        "{quarantine:?}"
    );
    assert!(matches!(history, Response::History { .. }), "{history:?}");
    drop(client);
    assert_eq!(
        launched.process.stopped().as_deref(),
        Some("it stopped with exit code 0")
    );
}

/// As the deb's app starts it. pkexec asks root for no password.
#[cfg(target_os = "linux")]
#[test]
fn starts_through_pkexec_when_elevated() {
    if !elevated_or_skip() {
        return;
    }
    if !std::path::Path::new("/usr/bin/pkexec").exists() {
        eprintln!("skipped: needs pkexec");
        return;
    }

    ask_quarantine_and_history(std::path::Path::new(env!("CARGO_BIN_EXE_cleanse-elevate")));
}

/// Windows looks for the DLLs a program imports beside it first. The helper's link flags make it
/// look in System32 only, so copies of a DLL without what it imports, planted beside it under the
/// names of the DLLs it imports from outside Windows' known DLLs, don't stop it.
#[cfg(windows)]
#[test]
fn ignores_dlls_planted_beside_it_when_elevated() {
    use cleanse_engine::SystemFolders;

    if !elevated_or_skip() {
        return;
    }
    let system32 = SystemFolders::current()
        .unwrap()
        .system_root
        .unwrap()
        .join("System32");
    let folder = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let helper = folder.path().join("cleanse-elevate.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_cleanse-elevate"), &helper).unwrap();
    for name in [
        "bcryptprimitives.dll",
        "cldapi.dll",
        "pdh.dll",
        "powrprof.dll",
        "userenv.dll",
        "vcruntime140.dll",
    ] {
        std::fs::copy(system32.join("version.dll"), folder.path().join(name)).unwrap();
    }

    let output = Command::new(&helper).stdin(Stdio::null()).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    // It ran, and said why it can't serve, rather than failing to load.
    assert!(stderr.contains("--pipe <name> --parent"), "{output:?}");
}
