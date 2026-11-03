//! What the web view can ask for. Each command is also listed in `build.rs` and allowed in
//! `capabilities/main.json`, or the web view can't call it.
//!
//! None takes a path. A scan covers the folders chosen in the system's folder picker, or the home
//! folder, and everything after that is named by an ID the session gave out.

#![allow(
    clippy::needless_pass_by_value,
    reason = "Tauri hands commands their arguments, `State` among them, by value"
)]

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use cleanse_engine::{Cleaned, Probe, ScanId, Section, Session, SessionError};
use cleanse_model::{self as model, CandidateId, Outcome, RuleId};
use rfd::AsyncFileDialog;
use tauri::ipc::{Channel, IpcResponse};
use tauri::{Runtime, State, WebviewWindow};

use crate::AppState;
use crate::views::{
    self, Amount, AppInfo, Choice, CleanReport, Details, Folders, Item, ItemPage, Problem, Results,
    ScanProgress, lossy,
};

#[tauri::command]
pub(crate) fn app_info(state: State<'_, AppState>) -> Result<AppInfo, String> {
    let session = state.session()?;
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        elevated: session.engine().system().is_elevated(),
    })
}

/// The folders a scan covers.
#[tauri::command(async)]
pub(crate) fn folders(state: State<'_, AppState>) -> Result<Folders, String> {
    folders_of(&state)
}

/// Opens the system's folder picker over the window, and adds the folders chosen to those a scan
/// covers.
#[tauri::command]
pub(crate) async fn pick_folders<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, AppState>,
) -> Result<Folders, String> {
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let parent = window.clone();
    // macOS and GTK only open dialogs from the main thread, as Tauri's dialog plugin knows too.
    window
        .run_on_main_thread(move || {
            let picking = AsyncFileDialog::new()
                .set_title("Choose folders to scan")
                .set_parent(&parent)
                .pick_folders();
            // Sending only fails once the command has stopped waiting, so there's no one to tell.
            let _ = sender.try_send(picking);
        })
        .map_err(message)?;
    let picking = receiver
        .recv()
        .await
        .ok_or("The folder picker couldn't open")?;
    if let Some(picked) = picking.await {
        state.choose(picked.iter().map(|folder| folder.path().to_path_buf()));
    }
    folders_of(&state)
}

/// Stops covering the chosen folder at `index` in [`folders`]' list.
#[tauri::command(async)]
pub(crate) fn remove_folder(state: State<'_, AppState>, index: usize) -> Result<Folders, String> {
    state.unchoose(index)?;
    folders_of(&state)
}

/// Scans the folders, reporting progress, and returns what it found. The results replace the
/// last scan's.
#[tauri::command]
pub(crate) async fn scan(
    state: State<'_, AppState>,
    progress: Channel<ScanProgress>,
) -> Result<Results, String> {
    let session = state.session()?;
    let roots = state.chosen();
    run(move || {
        let throttle = Throttle::default();
        let scan = session.scan(&roots, &|update| {
            if throttle.due() {
                tell(&progress, ScanProgress::from(update));
            }
        })?;
        results(&session, scan)
    })
    .await
}

/// Asks the scan or cleanup running now to stop.
#[tauri::command(async)]
pub(crate) fn cancel(state: State<'_, AppState>) -> Result<(), String> {
    state.session()?.cancel();
    Ok(())
}

/// A scan's results as they are now, after any cleanups.
#[tauri::command(async)]
pub(crate) fn summary(state: State<'_, AppState>, scan: u64) -> Result<Results, String> {
    let session = state.session()?;
    results(&session, ScanId(scan)).map_err(message)
}

/// The items in `section`, and of `rule` if given, largest first.
#[tauri::command(async)]
pub(crate) fn candidates(
    state: State<'_, AppState>,
    scan: u64,
    section: views::Section,
    rule: Option<String>,
    offset: usize,
    limit: usize,
) -> Result<ItemPage, String> {
    let session = state.session()?;
    let rule = rule.map(RuleId);
    let page = session
        .candidates(ScanId(scan), section.into(), rule.as_ref(), offset, limit)
        .map_err(message)?;
    let rules = session.engine().rules();
    Ok(ItemPage {
        items: page
            .items
            .iter()
            .map(|candidate| Item::new(candidate, rules))
            .collect(),
        offset: page.offset,
        total: page.total,
    })
}

#[tauri::command(async)]
pub(crate) fn explain(state: State<'_, AppState>, scan: u64, item: u64) -> Result<Details, String> {
    let session = state.session()?;
    let explained = session
        .explain(ScanId(scan), CandidateId(item))
        .map_err(message)?;
    Ok(Details::new(&explained, session.engine().rules()))
}

/// Removes the items `items` names, each the way its tier says, after checking each again, and
/// reports what happened to each. What's removed leaves the results.
#[tauri::command]
pub(crate) async fn clean(
    state: State<'_, AppState>,
    scan: u64,
    items: Vec<u64>,
    progress: Channel<usize>,
) -> Result<CleanReport, String> {
    let session = state.session()?;
    run(move || {
        let scan = ScanId(scan);
        let ids: Vec<CandidateId> = items.into_iter().map(CandidateId).collect();
        let throttle = Throttle::default();
        let cleaned = session.clean(scan, &ids, &|done| {
            if throttle.due() {
                tell(&progress, done);
            }
        })?;
        report(&session, scan, cleaned)
    })
    .await
}

fn folders_of(state: &AppState) -> Result<Folders, String> {
    let chosen = state.chosen();
    if !chosen.is_empty() {
        return Ok(Folders {
            folders: chosen.iter().map(|folder| lossy(folder)).collect(),
            home: false,
        });
    }
    let session = state.session()?;
    Ok(Folders {
        folders: session
            .engine()
            .default_roots()
            .iter()
            .map(|folder| lossy(folder))
            .collect(),
        home: true,
    })
}

/// Runs `work`, which scans or removes and takes a while, away from the threads that answer the
/// web view.
async fn run<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, SessionError> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(message)?
        .map_err(message)
}

fn results(session: &Session, scan: ScanId) -> Result<Results, SessionError> {
    let summary = session.summary(scan)?;
    let preselected = session
        .candidates(scan, Section::Removable, None, 0, usize::MAX)?
        .items
        .iter()
        .filter(|candidate| candidate.preselected())
        .filter_map(Choice::of)
        .collect();
    Ok(Results::new(summary, session.engine().rules(), preselected))
}

fn report(session: &Session, scan: ScanId, cleaned: Cleaned) -> Result<CleanReport, SessionError> {
    let mut deleted = (0, model::Size::default());
    let mut quarantined = (0, model::Size::default());
    let mut skipped = Vec::new();
    let mut failed = Vec::new();
    for (id, outcome) in cleaned.outcomes {
        match outcome {
            Outcome::Deleted { size } => {
                deleted.0 += 1;
                deleted.1 += size;
            }
            Outcome::Quarantined { size, .. } => {
                quarantined.0 += 1;
                quarantined.1 += size;
            }
            Outcome::Skipped { reason } => skipped.push((id, reason.to_string())),
            Outcome::Failed { error } => failed.push((id, error)),
        }
    }

    // What wasn't removed is still in the results, so that's where its path is.
    let paths = if skipped.is_empty() && failed.is_empty() {
        HashMap::new()
    } else {
        removable_paths(session, scan)?
    };
    let problems = |list: Vec<(CandidateId, String)>| -> Vec<Problem> {
        list.into_iter()
            .map(|(id, reason)| Problem {
                path: paths
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| format!("Item {}", id.0)),
                reason,
            })
            .collect()
    };
    let amount = |(items, size): (usize, model::Size)| Amount {
        items,
        size: size.into(),
    };
    Ok(CleanReport {
        deleted: amount(deleted),
        quarantined: amount(quarantined),
        skipped: problems(skipped),
        failed: problems(failed),
        journal: cleaned.journal.as_deref().map(lossy),
    })
}

/// The paths of everything a cleanup could have been given, by ID.
fn removable_paths(
    session: &Session,
    scan: ScanId,
) -> Result<HashMap<CandidateId, String>, SessionError> {
    let mut paths = HashMap::new();
    for section in [Section::Removable, Section::Review] {
        let page = session.candidates(scan, section, None, 0, usize::MAX)?;
        paths.extend(
            page.items
                .iter()
                .map(|candidate| (candidate.id, lossy(&candidate.found.path))),
        );
    }
    Ok(paths)
}

/// Sends progress to the web view. It's only progress, so if the window has closed, there's no
/// one to tell.
fn tell<T: IpcResponse>(channel: &Channel<T>, update: T) {
    let _ = channel.send(update);
}

fn message(error: impl Display) -> String {
    error.to_string()
}

/// Lets an update through at most every [`Throttle::INTERVAL`], so a fast scan doesn't flood
/// the web view.
#[derive(Default)]
struct Throttle(Mutex<Option<Instant>>);

impl Throttle {
    const INTERVAL: Duration = Duration::from_millis(100);

    fn due(&self) -> bool {
        let mut last = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        if last.is_some_and(|last| now.duration_since(last) < Self::INTERVAL) {
            return false;
        }
        *last = Some(now);
        true
    }
}
