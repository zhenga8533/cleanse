//! System cleanup in the app (ADR 0010). The window never runs elevated, so these commands go to
//! the elevated helper, which the first that needs it starts through the UAC prompt or pkexec.
//! Each is also listed in `build.rs` and allowed in `capabilities/main.json`.
//!
//! Like the app's other commands, none takes a path. Scans and items are named by the IDs the
//! helper's session gave out, and quarantine entries by the IDs the helper listed.

#![allow(
    clippy::needless_pass_by_value,
    reason = "Tauri hands commands their arguments, `State` among them, by value"
)]

use std::fmt::Display;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use cleanse_desktop_views::{
    self as views, CleanProgress, CleanReport, Details, HistoryView, ItemPage, QuarantineView,
    Results, RuleView, ScanProgress, SystemStatus,
};
use cleanse_elevate::client::Client;
use cleanse_elevate::protocol::{Request, Response};
use cleanse_engine::helper::{self, HelperProcess};
use cleanse_engine::{Elevation, Engine, Probe, System};
use tauri::ipc::Channel;
use tauri::{Runtime, State, WebviewWindow};

use crate::commands::{Throttle, message, tell};

type Answers = Box<dyn BufRead + Send>;
type Requests = Box<dyn Write + Send>;

/// A helper's two ends, and its process if it's a program of its own.
pub(crate) struct Connection {
    pub(crate) answers: Answers,
    pub(crate) requests: Requests,
    pub(crate) process: Option<HelperProcess>,
}

/// How the app starts a helper.
pub(crate) trait Starter: Send + Sync {
    /// Why it can't, if it can't.
    fn unavailable(&self) -> Option<String>;

    /// Starts one, asking for administrator rights over the window whose `HWND` is `window`.
    fn start(&self, window: Option<usize>) -> Result<Connection, String>;
}

/// The helper installed with the app.
struct Installed;

impl Installed {
    fn find() -> Result<helper::Installed, String> {
        let system = System::new().map_err(message)?;
        match system.elevation() {
            Elevation::None => helper::find(&system).map_err(message),
            Elevation::Elevated => Err("Cleanse is running as an administrator or root. Open it \
                 again without elevation, and system cleanup will ask for administrator rights \
                 itself."
                .to_owned()),
            Elevation::AlwaysElevated => Err("Windows runs everything this account starts as an \
                 administrator, as it does with UAC off, and the app doesn't start its helper \
                 from an elevated window."
                .to_owned()),
        }
    }
}

impl Starter for Installed {
    fn unavailable(&self) -> Option<String> {
        Self::find().err()
    }

    fn start(&self, window: Option<usize>) -> Result<Connection, String> {
        let launched = helper::launch(&Self::find()?, window).map_err(message)?;
        Ok(Connection {
            answers: launched.answers,
            requests: launched.requests,
            process: Some(launched.process),
        })
    }
}

/// The helper the app has started, if it's still running.
pub(crate) struct SystemState {
    starter: Box<dyn Starter>,
    /// Held while a helper starts, so only one does.
    starting: Mutex<()>,
    helper: Mutex<Option<Arc<Helper>>>,
}

struct Helper {
    client: Client<Answers, Requests>,
    process: Mutex<Option<HelperProcess>>,
}

impl Helper {
    /// Whether its process has stopped, as it does after ten minutes with nothing asked.
    fn has_stopped(&self) -> bool {
        lock(&self.process)
            .as_mut()
            .is_some_and(HelperProcess::has_stopped)
    }

    /// Why it stopped, or `error`, which said that it had.
    fn why_stopped(&self, error: impl Display) -> String {
        lock(&self.process)
            .as_mut()
            .and_then(HelperProcess::stopped)
            .unwrap_or_else(|| error.to_string())
    }
}

impl SystemState {
    /// With the helper installed with the app.
    pub(crate) fn installed() -> Arc<Self> {
        Self::with(Box::new(Installed))
    }

    pub(crate) fn with(starter: Box<dyn Starter>) -> Arc<Self> {
        Arc::new(Self {
            starter,
            starting: Mutex::default(),
            helper: Mutex::default(),
        })
    }

    fn running(&self) -> Option<Arc<Helper>> {
        lock(&self.helper)
            .clone()
            .filter(|helper| !helper.has_stopped())
    }

    /// The running helper, or a new one.
    fn helper(&self, window: Option<usize>) -> Result<Arc<Helper>, String> {
        let _starting = lock(&self.starting);
        if let Some(helper) = self.running() {
            return Ok(helper);
        }
        let connection = self.starter.start(window)?;
        let mut process = connection.process;
        let client = Client::connect(connection.answers, connection.requests).map_err(|error| {
            let why = process.as_mut().and_then(HelperProcess::stopped);
            format!(
                "System cleanup didn't start: {}",
                why.unwrap_or_else(|| error.to_string())
            )
        })?;
        let helper = Arc::new(Helper {
            client,
            process: Mutex::new(process),
        });
        *lock(&self.helper) = Some(Arc::clone(&helper));
        Ok(helper)
    }

    /// Asks the helper, starting one if none is running, and passes it progress. Its errors, such
    /// as an ID from a scan it no longer has, are returned as they are.
    fn ask(
        &self,
        window: Option<usize>,
        request: &Request,
        progress: &mut dyn FnMut(Response),
    ) -> Result<Response, String> {
        let helper = self.helper(window)?;
        match helper.client.ask(request, progress) {
            Ok(Response::Error { message }) => Err(message),
            Ok(response) => Ok(response),
            Err(error) => {
                let mut known = lock(&self.helper);
                if known
                    .as_ref()
                    .is_some_and(|known| Arc::ptr_eq(known, &helper))
                {
                    *known = None;
                }
                drop(known);
                Err(format!(
                    "System cleanup stopped: {}",
                    helper.why_stopped(error)
                ))
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The window's `HWND` on Windows, which the UAC prompt belongs to.
fn owner<R: Runtime>(window: &WebviewWindow<R>) -> Option<usize> {
    #[cfg(windows)]
    {
        window.hwnd().ok().map(|hwnd| hwnd.0.addr())
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

/// Runs `work`, which waits on the helper and maybe the UAC prompt, away from the threads that
/// answer the web view.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(message)?
}

/// Asks the helper for `request` from a command, away from the threads that answer the web view,
/// and hands its response to `answer`.
async fn ask<R: Runtime, T: Send + 'static>(
    window: &WebviewWindow<R>,
    state: &State<'_, Arc<SystemState>>,
    request: Request,
    answer: fn(Response) -> Option<T>,
) -> Result<T, String> {
    let system = Arc::clone(state);
    let window = owner(window);
    blocking(move || {
        let response = system.ask(window, &request, &mut |_| {})?;
        answer_or_err(&request, response, answer)
    })
    .await
}

fn answer_or_err<T>(
    request: &Request,
    response: Response,
    answer: fn(Response) -> Option<T>,
) -> Result<T, String> {
    let shown = format!("{response:?}");
    answer(response)
        .ok_or_else(|| format!("System cleanup answered {request:?} with something else: {shown}"))
}

/// Whether the app can do system cleanup, and whether its helper is running.
#[tauri::command(async)]
pub(crate) fn system_status(state: State<'_, Arc<SystemState>>) -> SystemStatus {
    let running = state.running().is_some();
    SystemStatus {
        supported: cfg!(any(windows, target_os = "linux")),
        unavailable: if running {
            None
        } else {
            state.starter.unavailable()
        },
        running,
    }
}

/// Scans the system's folders, reporting progress, and returns what it found. The results
/// replace the last system scan's.
#[tauri::command]
pub(crate) async fn system_scan<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<SystemState>>,
    progress: Channel<ScanProgress>,
) -> Result<Results, String> {
    let system = Arc::clone(&state);
    let window = owner(&window);
    blocking(move || {
        let throttle = Throttle::default();
        let request = Request::Scan {};
        let response = system.ask(window, &request, &mut |response| {
            if let Response::ScanProgress { progress: update } = response
                && throttle.due()
            {
                tell(&progress, update);
            }
        })?;
        answer_or_err(&request, response, |response| match response {
            Response::Results { results } => Some(results),
            _ => None,
        })
    })
    .await
}

/// Asks the system scan or cleanup running now to stop. Without a helper, nothing is running.
#[tauri::command(async)]
pub(crate) fn system_cancel(state: State<'_, Arc<SystemState>>) -> Result<(), String> {
    match state.running() {
        Some(helper) => helper.client.cancel().map_err(message),
        None => Ok(()),
    }
}

/// A system scan's results as they are now, after any cleanups.
#[tauri::command]
pub(crate) async fn system_summary<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<SystemState>>,
    scan: u64,
) -> Result<Results, String> {
    ask(
        &window,
        &state,
        Request::Summary { scan },
        |response| match response {
            Response::Results { results } => Some(results),
            _ => None,
        },
    )
    .await
}

/// The items in `section`, and of `rule` if given, largest first.
#[tauri::command]
pub(crate) async fn system_candidates<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<SystemState>>,
    scan: u64,
    section: views::Section,
    rule: Option<String>,
    offset: usize,
    limit: usize,
) -> Result<ItemPage, String> {
    let request = Request::Candidates {
        scan,
        section,
        rule,
        offset,
        limit,
    };
    ask(&window, &state, request, |response| match response {
        Response::Page { page } => Some(page),
        _ => None,
    })
    .await
}

#[tauri::command]
pub(crate) async fn system_explain<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<SystemState>>,
    scan: u64,
    item: u64,
) -> Result<Details, String> {
    ask(
        &window,
        &state,
        Request::Explain { scan, item },
        |response| match response {
            Response::Details { details } => Some(details),
            _ => None,
        },
    )
    .await
}

/// Removes the items `items` names, each the way its tier says, after the helper checks each
/// again, and reports what happened to each.
#[tauri::command]
pub(crate) async fn system_clean<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<SystemState>>,
    scan: u64,
    items: Vec<u64>,
    progress: Channel<CleanProgress>,
) -> Result<CleanReport, String> {
    let system = Arc::clone(&state);
    let window = owner(&window);
    blocking(move || {
        let request = Request::Clean { scan, items };
        // The helper paces these, so each one is passed on.
        let response = system.ask(window, &request, &mut |response| {
            if let Response::CleanProgress { progress: update } = response {
                tell(&progress, update);
            }
        })?;
        answer_or_err(&request, response, |response| match response {
            Response::Cleaned { report } => Some(report),
            _ => None,
        })
    })
    .await
}

/// What's in system cleanup's quarantine, oldest first.
#[tauri::command]
pub(crate) async fn system_quarantined<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<SystemState>>,
) -> Result<QuarantineView, String> {
    ask(
        &window,
        &state,
        Request::Quarantined {},
        |response| match response {
            Response::Quarantine { quarantine } => Some(quarantine),
            _ => None,
        },
    )
    .await
}

/// Puts back the quarantined items `ids` names, unless something has taken their place.
#[tauri::command]
pub(crate) async fn system_restore<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<SystemState>>,
    ids: Vec<String>,
) -> Result<views::Handled, String> {
    ask(&window, &state, Request::Restore { ids }, handled).await
}

/// Deletes the quarantined items `ids` names for good.
#[tauri::command]
pub(crate) async fn system_purge<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<SystemState>>,
    ids: Vec<String>,
) -> Result<views::Handled, String> {
    ask(&window, &state, Request::Purge { ids }, handled).await
}

/// What system cleanup has done before, most recent first, and what's left of removals that
/// didn't finish.
#[tauri::command]
pub(crate) async fn system_history<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<SystemState>>,
) -> Result<HistoryView, String> {
    ask(
        &window,
        &state,
        Request::History {},
        |response| match response {
            Response::History { history } => Some(history),
            _ => None,
        },
    )
    .await
}

/// Removes what's left of every system removal that didn't finish, if it's still what was
/// approved.
#[tauri::command]
pub(crate) async fn system_finish<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<SystemState>>,
) -> Result<views::Handled, String> {
    ask(&window, &state, Request::Finish {}, handled).await
}

/// System cleanup's rules, as `cleanse system rules` lists them. They're built in, so reading
/// them needs no helper.
#[tauri::command(async)]
pub(crate) fn system_rules() -> Result<Vec<RuleView>, String> {
    let engine = Engine::new_system().map_err(message)?;
    Ok(engine
        .rules()
        .rules()
        .iter()
        .map(|rule| RuleView::new(rule, engine.config().large_file_size))
        .collect())
}

fn handled(response: Response) -> Option<views::Handled> {
    match response {
        Response::Handled { handled } => Some(handled),
        _ => None,
    }
}
