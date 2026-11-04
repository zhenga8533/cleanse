//! What the web view can ask for. Each command is also listed in `build.rs` and allowed in
//! `capabilities/main.json`, or the web view can't call it.
//!
//! None takes a path. A scan covers the folders chosen in the system's folder picker, or the home
//! folder, and everything after that is named by an ID the session gave out.

#![allow(
    clippy::needless_pass_by_value,
    reason = "Tauri hands commands their arguments, `State` among them, by value"
)]

use std::fmt::Display;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use cleanse_desktop_views::{
    self as views, AppInfo, CleanReport, Details, EntryItem, Folders, HistoryView, ItemPage,
    KnownFolder, QuarantineView, Results, RuleView, ScanProgress, SettingsView, lossy,
};
use cleanse_engine::{FolderEntry, Probe, ScanId, SessionError, cloud_sync_detection};
use cleanse_model::{CandidateId, Category, RuleId};
use rfd::AsyncFileDialog;
use tauri::ipc::{Channel, IpcResponse};
use tauri::{Runtime, State, WebviewWindow};

use crate::AppState;

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
        views::results(&session, scan)
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
    views::results(&session, ScanId(scan)).map_err(message)
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
    views::item_page(
        &session,
        ScanId(scan),
        section,
        rule.as_ref(),
        offset,
        limit,
    )
    .map_err(message)
}

#[tauri::command(async)]
pub(crate) fn explain(state: State<'_, AppState>, scan: u64, item: u64) -> Result<Details, String> {
    let session = state.session()?;
    let explained = session
        .explain(ScanId(scan), CandidateId(item))
        .map_err(message)?;
    Ok(Details::new(&explained, session.engine().rules()))
}

/// The folder `node` names, or the scan's roots, with its largest entries up to `limit`. Only
/// items in `categories` count as removable, or every item if there are none.
#[tauri::command(async)]
pub(crate) fn folder(
    state: State<'_, AppState>,
    scan: u64,
    node: Option<u32>,
    limit: usize,
    categories: Vec<String>,
) -> Result<views::FolderView, String> {
    let session = state.session()?;
    let scan = ScanId(scan);
    let categories = categories
        .iter()
        .map(|id| {
            Category::ALL
                .into_iter()
                .find(|category| category.id() == id)
                .ok_or_else(|| format!("There's no category {id}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let view = session
        .folder(scan, node, limit, &categories)
        .map_err(message)?;
    let rules = session.engine().rules();
    let entry = |entry: FolderEntry| -> Result<views::FolderEntry, String> {
        let item = entry
            .candidate
            .map(|id| session.explain(scan, id))
            .transpose()
            .map_err(message)?
            .map(|explained| EntryItem::new(&explained, rules));
        Ok(views::FolderEntry::new(&entry, item))
    };
    Ok(views::FolderView {
        trail: view
            .trail
            .into_iter()
            .map(entry)
            .collect::<Result<_, _>>()?,
        entries: view
            .entries
            .into_iter()
            .map(entry)
            .collect::<Result<_, _>>()?,
        hidden: view.hidden,
        hidden_on_disk: view.hidden_on_disk,
        own_files: view.own_files,
        own_usage: view.own_usage.into(),
        own_removable: view.own_removable,
    })
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
        views::clean_report(&session, scan, cleaned)
    })
    .await
}

/// What's in quarantine, oldest first.
#[tauri::command(async)]
pub(crate) fn quarantined(state: State<'_, AppState>) -> Result<QuarantineView, String> {
    let session = state.session()?;
    let listing = session.quarantined().map_err(message)?;
    Ok(QuarantineView::new(&listing, session.engine().rules()))
}

/// Puts back the quarantined items `ids` names, unless something has taken their place.
#[tauri::command]
pub(crate) async fn restore(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<views::Handled, String> {
    let session = state.session()?;
    run(move || session.restore(&ids).map(views::Handled::from)).await
}

/// Deletes the quarantined items `ids` names for good.
#[tauri::command]
pub(crate) async fn purge(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<views::Handled, String> {
    let session = state.session()?;
    run(move || session.purge(&ids).map(views::Handled::from)).await
}

/// What Cleanse has done before, most recent first, and what's left of removals that didn't
/// finish.
#[tauri::command(async)]
pub(crate) fn history(state: State<'_, AppState>) -> Result<HistoryView, String> {
    let session = state.session()?;
    let history = session.history().map_err(message)?;
    Ok(HistoryView::from(&history))
}

/// Removes what's left of every removal that didn't finish, if it's still what was approved.
#[tauri::command]
pub(crate) async fn finish(state: State<'_, AppState>) -> Result<views::Handled, String> {
    let session = state.session()?;
    run(move || session.finish().map(views::Handled::from)).await
}

/// How Cleanse is set up on this system, as `cleanse doctor` reports it.
#[tauri::command(async)]
pub(crate) fn settings(state: State<'_, AppState>) -> Result<SettingsView, String> {
    let session = state.session()?;
    let engine = session.engine();
    let config = engine.config();
    let folders = engine.system().known_folders();
    let folder = |name: &str, path: Option<&Path>| KnownFolder {
        name: name.to_owned(),
        path: path.map(lossy),
    };
    Ok(SettingsView {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        elevated: engine.system().is_elevated(),
        path: engine.config_path().map(lossy),
        exists: engine.config_path().is_some_and(Path::exists),
        exclude: config.exclude.iter().map(|path| lossy(path)).collect(),
        disabled_rules: config
            .disabled_rules
            .iter()
            .map(|rule| rule.0.clone())
            .collect(),
        large_file_size: config.large_file_size,
        cloud_sync_detection: cloud_sync_detection().to_owned(),
        folders: vec![
            folder("Home", folders.home.as_deref()),
            folder("Documents", folders.documents.as_deref()),
            folder("Downloads", folders.downloads.as_deref()),
            folder("Cache", folders.cache.as_deref()),
            folder("App data", folders.data_local.as_deref()),
            folder("Settings", folders.config.as_deref()),
            folder("Temp", Some(&folders.temp)),
        ],
    })
}

/// Every rule the settings don't turn off, as `cleanse rules show` describes each.
#[tauri::command(async)]
pub(crate) fn rules(state: State<'_, AppState>) -> Result<Vec<RuleView>, String> {
    let session = state.session()?;
    let engine = session.engine();
    Ok(engine
        .rules()
        .rules()
        .iter()
        .map(|rule| RuleView::new(rule, engine.config().large_file_size))
        .collect())
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
