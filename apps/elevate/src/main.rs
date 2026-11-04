//! The elevated helper's program. The desktop app starts it as an administrator or root to do
//! system cleanup for its window, which never runs elevated (ADR 0010).

use std::error::Error;
use std::process::ExitCode;
use std::sync::Arc;

use cleanse_elevate::server::{self, Ending};
use cleanse_engine::{Engine, Probe, Session};

fn main() -> ExitCode {
    match run() {
        Ok(Ending::Closed | Ending::Idle) => ExitCode::SUCCESS,
        Ok(ending) => {
            eprintln!("cleanse-elevate: stopped: {ending:?}");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("cleanse-elevate: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<Ending, Box<dyn Error>> {
    #[cfg(windows)]
    cleanse_platform::privileged::load_dlls_from_system32_only()?;
    if cfg!(target_os = "macos") {
        return Err("system cleanup is only for Windows and Linux".into());
    }
    let engine = Engine::new_system()?;
    if !engine.system().is_elevated() {
        return Err("it runs only as an administrator or root".into());
    }
    serve(&Arc::new(Session::new(engine)))
}

/// pkexec starts it with pipes from the window as its standard input and output.
#[cfg(not(windows))]
fn serve(session: &Arc<Session>) -> Result<Ending, Box<dyn Error>> {
    use std::io::{self, BufReader};

    if std::env::args_os().len() > 1 {
        return Err("it takes no arguments here".into());
    }
    Ok(server::serve(
        session,
        BufReader::new(io::stdin()),
        io::stdout(),
        server::IDLE,
    ))
}

/// Windows' UAC prompt can't pass pipes on, as pkexec does, so the window names pipes for the
/// helper to make, which only the window may open.
#[cfg(windows)]
fn serve(session: &Arc<Session>) -> Result<Ending, Box<dyn Error>> {
    use std::io::BufReader;

    use cleanse_platform::privileged::pipe::Parent;

    let (name, parent) = windows::arguments()?;
    // Installed beside the desktop app's program, which is the only one it serves.
    let program = std::env::current_exe()?.with_file_name(windows::DESKTOP);
    let parent = Parent::check(parent, &program)
        .map_err(|error| format!("checking the window's process: {error}"))?;
    let connection = windows::accept(name, parent)?;
    Ok(server::serve(
        session,
        BufReader::new(connection.requests),
        connection.answers,
        server::IDLE,
    ))
}

#[cfg(windows)]
mod windows {
    use std::error::Error;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::thread;
    use std::time::Duration;

    use cleanse_platform::privileged::pipe::{self, Connection, Parent};

    /// The desktop app's program.
    pub const DESKTOP: &str = "cleanse-desktop.exe";
    /// How long the window has to connect once the helper starts.
    const CONNECT: Duration = Duration::from_secs(30);
    const USAGE: &str = "the desktop app starts it with --pipe <name> --parent <process ID>";

    /// The name of the pipes to make, and the window's process ID.
    pub fn arguments() -> Result<(String, u32), Box<dyn Error>> {
        let arguments: Vec<_> = std::env::args_os().skip(1).collect();
        let [pipe, name, parent, pid] = arguments.as_slice() else {
            return Err(USAGE.into());
        };
        if pipe != "--pipe" || parent != "--parent" {
            return Err(USAGE.into());
        }
        let name = name.to_str().ok_or(USAGE)?;
        let pid = pid.to_str().and_then(|pid| pid.parse().ok()).ok_or(USAGE)?;
        Ok((name.to_owned(), pid))
    }

    /// Makes the pipes named `name`, and waits for `parent` to connect to them.
    pub fn accept(name: String, parent: Parent) -> Result<Connection, Box<dyn Error>> {
        let (sender, served) = mpsc::channel();
        // Not joined: if the window never connects, it waits on the pipes until the program
        // exits.
        thread::spawn(move || {
            #[expect(
                clippy::disallowed_methods,
                reason = "the elevated helper makes its pipes (ADR 0010)"
            )]
            let connection = pipe::serve(&name, &parent);
            // Once the time to connect has run out, no one is waiting for it.
            let _ = sender.send(connection);
        });
        match served.recv_timeout(CONNECT) {
            Ok(connection) => {
                connection.map_err(|error| format!("serving the window's pipes: {error}").into())
            }
            Err(RecvTimeoutError::Timeout) => Err(format!(
                "the window didn't connect within {} seconds",
                CONNECT.as_secs()
            )
            .into()),
            Err(RecvTimeoutError::Disconnected) => Err("making the window's pipes failed".into()),
        }
    }
}
