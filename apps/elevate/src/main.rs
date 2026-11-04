//! The elevated helper's program. The desktop app starts it as an administrator or root to do
//! system cleanup for its window, which never runs elevated (ADR 0010).

use std::error::Error;
use std::io::{self, BufReader};
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
    let engine = Engine::new_system()?;
    if !engine.system().is_elevated() {
        return Err("it runs only as an administrator or root".into());
    }
    // On Linux, pkexec starts it with pipes from the window as its standard input and output.
    // Windows' UAC prompt can't pass pipes on, so there it needs a pipe of its own.
    if !cfg!(target_os = "linux") {
        return Err("it can only talk to the window on Linux so far".into());
    }
    let session = Arc::new(Session::new(engine));
    Ok(server::serve(
        &session,
        BufReader::new(io::stdin()),
        io::stdout(),
        server::IDLE,
    ))
}
