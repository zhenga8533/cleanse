//! Starting the desktop app's elevated helper, which does system cleanup for the app's window
//! (ADR 0010). The app starts it only from where its installer put it, where only
//! administrators or root can change it, so no one else can have a program of theirs run
//! elevated in its place.

use std::ffi::OsStr;
use std::fmt;
use std::io::{self, BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use cleanse_model::EntryKind;
use cleanse_platform::Probe;

/// The helper's program on Windows, which the installer puts beside the app's.
const WINDOWS_PROGRAM: &str = "cleanse-elevate.exe";
const PKEXEC: &str = "/usr/bin/pkexec";
/// How long the helper has to say why it stopped once its answers end.
const STOPPING: Duration = Duration::from_secs(1);

/// The helper, installed where only administrators or root can change it.
#[derive(Clone, Debug)]
pub struct Installed {
    program: PathBuf,
}

impl Installed {
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The helper at `program`, unchecked, for tests that start a copy of it.
    #[cfg(feature = "fake")]
    pub fn unchecked(program: PathBuf) -> Self {
        Self { program }
    }
}

/// Why the app can't start the helper.
#[derive(Debug)]
pub enum Unavailable {
    /// System cleanup is only for Windows and Linux.
    Unsupported,
    /// This copy of the app wasn't installed with the helper, as the AppImage, a copy run from
    /// elsewhere, or a development build isn't.
    NotInstalled,
    /// pkexec, which starts the helper as root, isn't installed.
    NoPkexec,
    /// Someone other than administrators or root could change `path`, so what's there might not
    /// be what was installed.
    Unprotected(PathBuf),
    /// Where this copy of the app is couldn't be told.
    Unlocated(io::Error),
    /// `path` couldn't be checked.
    Unchecked { path: PathBuf, error: io::Error },
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("system cleanup is only for Windows and Linux"),
            Self::NotInstalled => {
                f.write_str("this copy of Cleanse wasn't installed with its system cleanup helper")
            }
            Self::NoPkexec => write!(f, "pkexec, which runs system cleanup, isn't at {PKEXEC}"),
            Self::Unprotected(path) => write!(
                f,
                "people other than administrators or root can change {}",
                path.display()
            ),
            Self::Unlocated(error) => write!(f, "where Cleanse is installed is unknown: {error}"),
            Self::Unchecked { path, error } => {
                write!(f, "{} couldn't be checked: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for Unavailable {}

/// Finds the helper that was installed with this copy of the app: on Windows, beside this
/// program in Program Files, and on Linux, where the deb puts it.
pub fn find(probe: &dyn Probe) -> Result<Installed, Unavailable> {
    #[cfg(windows)]
    {
        let base = cleanse_platform::program_files().map_err(Unavailable::Unlocated)?;
        let app = std::env::current_exe().map_err(Unavailable::Unlocated)?;
        beside(probe, &base, &app)
    }
    #[cfg(target_os = "linux")]
    {
        installed_at(
            probe,
            // Where the deb installs it.
            Path::new("/usr/lib/cleanse/cleanse-elevate"),
            // What has pkexec ask for an administrator's password each time it starts it.
            Path::new("/usr/share/polkit-1/actions/io.github.zhenga8533.cleanse.policy"),
            Path::new(PKEXEC),
        )
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = probe;
        Err(Unavailable::Unsupported)
    }
}

/// The helper beside `app`, if `app` is in `base` and only administrators can change the helper
/// and every folder from `base` down to it. Whoever can change a folder can replace what's in it.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "Windows' layout, tested everywhere")
)]
fn beside(probe: &dyn Probe, base: &Path, app: &Path) -> Result<Installed, Unavailable> {
    let folder = app.parent().ok_or(Unavailable::NotInstalled)?;
    let names = names_below(base, folder).ok_or(Unavailable::NotInstalled)?;
    let program = folder.join(WINDOWS_PROGRAM);
    present(probe, &program)?;
    // `base` as the app's path spells it.
    let mut path: PathBuf = folder
        .components()
        .take(base.components().count())
        .collect();
    protected(probe, &path)?;
    for name in names {
        path.push(name);
        protected(probe, &path)?;
    }
    protected(probe, &program)?;
    Ok(Installed { program })
}

/// The names of the folders from `base` down to `path`, if `path` is in `base`. Windows paths
/// ignore case.
fn names_below<'a>(base: &Path, path: &'a Path) -> Option<Vec<&'a OsStr>> {
    let mut parts = path.components();
    for part in base.components() {
        if !parts
            .next()?
            .as_os_str()
            .eq_ignore_ascii_case(part.as_os_str())
        {
            return None;
        }
    }
    parts
        .map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect()
}

/// The helper at `program`, with the polkit policy at `policy`, if pkexec is there to start it,
/// and only root can change either of them or any folder above them.
#[cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "Linux's layout, tested everywhere")
)]
fn installed_at(
    probe: &dyn Probe,
    program: &Path,
    policy: &Path,
    pkexec: &Path,
) -> Result<Installed, Unavailable> {
    present(probe, program)?;
    if probe.inspect(pkexec).is_err() {
        return Err(Unavailable::NoPkexec);
    }
    for path in [program, policy] {
        let mut ancestors: Vec<&Path> = path.ancestors().collect();
        ancestors.reverse();
        for ancestor in ancestors {
            protected(probe, ancestor)?;
        }
    }
    Ok(Installed {
        program: program.to_path_buf(),
    })
}

/// Checks that `program` is there.
fn present(probe: &dyn Probe, program: &Path) -> Result<(), Unavailable> {
    match probe.inspect(program) {
        Ok(facts) if facts.kind == EntryKind::File => Ok(()),
        Ok(_) => Err(Unavailable::NotInstalled),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(Unavailable::NotInstalled),
        Err(error) => Err(Unavailable::Unchecked {
            path: program.to_path_buf(),
            error,
        }),
    }
}

/// Checks that only administrators or root can change `path`.
fn protected(probe: &dyn Probe, path: &Path) -> Result<(), Unavailable> {
    match probe.only_admins_can_change(path) {
        Ok(true) => Ok(()),
        Ok(false) => Err(Unavailable::Unprotected(path.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(Unavailable::NotInstalled),
        Err(error) => Err(Unavailable::Unchecked {
            path: path.to_path_buf(),
            error,
        }),
    }
}

/// A helper this process started: requests go to it on `requests`, and its answers come back on
/// `answers`. It stops when `requests` closes.
pub struct Launched {
    pub answers: Box<dyn BufRead + Send>,
    pub requests: Box<dyn Write + Send>,
    pub process: HelperProcess,
}

/// The helper's process.
#[derive(Debug)]
pub struct HelperProcess {
    #[cfg(windows)]
    process: cleanse_platform::privileged::Process,
    #[cfg(unix)]
    child: Option<std::process::Child>,
}

impl HelperProcess {
    /// Whether it has stopped, without waiting.
    pub fn has_stopped(&mut self) -> bool {
        matches!(self.exit(), Ok(Some(_)))
    }

    /// Why it stopped, once it has, for the window to say. Its answers end as it exits, so this
    /// waits a moment for that.
    pub fn stopped(&mut self) -> Option<String> {
        let deadline = std::time::Instant::now() + STOPPING;
        loop {
            match self.exit() {
                Ok(Some(why)) => return Some(why),
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => return None,
                Err(error) => return Some(format!("its process couldn't be checked: {error}")),
            }
        }
    }

    #[cfg(windows)]
    fn exit(&mut self) -> io::Result<Option<String>> {
        Ok(self
            .process
            .exit_code()?
            .map(|code| format!("it stopped with exit code {code}")))
    }

    #[cfg(unix)]
    fn exit(&mut self) -> io::Result<Option<String>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        Ok(child.try_wait()?.map(|status| match status.code() {
            // pkexec's own: the password prompt was dismissed, or polkit refused.
            Some(126) => "the request for an administrator's password was dismissed".to_owned(),
            Some(127) => "polkit didn't allow it to run as root".to_owned(),
            Some(code) => format!("it stopped with exit code {code}"),
            None => format!("it was stopped: {status}"),
        }))
    }
}

#[cfg(unix)]
impl Drop for HelperProcess {
    fn drop(&mut self) {
        // It exits once its requests close, and waiting for it keeps it from lingering as a
        // zombie, without holding up whoever let go of it.
        if let Some(mut child) = self.child.take() {
            std::thread::spawn(move || child.wait());
        }
    }
}

/// Starts the helper as an administrator or root, which asks the user first: through the UAC
/// prompt on Windows, over the window whose `HWND` is `window` if given, or through pkexec on
/// Linux.
pub fn launch(installed: &Installed, window: Option<usize>) -> io::Result<Launched> {
    os_launch(installed, window)
}

#[cfg(windows)]
fn os_launch(installed: &Installed, window: Option<usize>) -> io::Result<Launched> {
    use std::hash::{BuildHasher, RandomState};
    use std::io::BufReader;
    use std::time::Instant;

    use cleanse_platform::privileged::{pipe, run_as_administrator};

    /// How long the helper has to make its pipes once Windows has started it.
    const CONNECT: Duration = Duration::from_secs(10);
    /// How long each try to connect waits, between checks that the helper is still running.
    const TRY: Duration = Duration::from_millis(250);

    // Others can't guess the name to take it first, since `RandomState`'s keys are random.
    let name = format!(
        "cleanse-{}-{:016x}",
        std::process::id(),
        RandomState::new().hash_one(Instant::now())
    );
    let parameters = format!("--pipe {name} --parent {}", std::process::id());
    #[expect(
        clippy::disallowed_methods,
        reason = "cleanse-exec starts the elevated helper (ADR 0010)"
    )]
    let process = run_as_administrator(&installed.program, &parameters, window)?;
    let deadline = Instant::now() + CONNECT;
    let connection = loop {
        #[expect(
            clippy::disallowed_methods,
            reason = "cleanse-exec connects to the elevated helper (ADR 0010)"
        )]
        let connected = pipe::connect(&name, process.id(), TRY);
        match connected {
            Ok(connection) => break connection,
            Err(error) => {
                if let Some(code) = process.exit_code()? {
                    return Err(io::Error::other(format!(
                        "the helper stopped with exit code {code} before it was ready"
                    )));
                }
                if error.kind() != io::ErrorKind::NotFound || Instant::now() >= deadline {
                    return Err(error);
                }
            }
        }
    };
    Ok(Launched {
        answers: Box::new(BufReader::new(connection.answers)),
        requests: Box::new(connection.requests),
        process: HelperProcess { process },
    })
}

#[cfg(unix)]
fn os_launch(installed: &Installed, _window: Option<usize>) -> io::Result<Launched> {
    use std::io::BufReader;
    use std::process::{Command, Stdio};

    if cfg!(not(target_os = "linux")) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            Unavailable::Unsupported.to_string(),
        ));
    }
    // pkexec passes its standard input and output on to the helper.
    #[expect(
        clippy::disallowed_methods,
        reason = "cleanse-exec starts the elevated helper (ADR 0010)"
    )]
    let mut child = Command::new(PKEXEC)
        .arg(&installed.program)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let (Some(requests), Some(answers)) = (child.stdin.take(), child.stdout.take()) else {
        return Err(io::Error::other("pkexec started without pipes"));
    };
    Ok(Launched {
        answers: Box::new(BufReader::new(answers)),
        requests: Box::new(requests),
        process: HelperProcess { child: Some(child) },
    })
}

#[cfg(test)]
mod tests;
