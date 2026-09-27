mod platform;
pub mod schedule;
#[cfg(test)]
mod tests;

use crate::workspace::{EntryKind, WorkspaceRef};
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use schedule::{Parsed, Reminder, Repeat};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Clone, Default, Serialize, Deserialize)]
struct NoteIndex {
    revision: Option<String>,
    checked_at: i64,
    parsed: Parsed,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct WorkspaceIndex {
    local_path: Option<PathBuf>,
    notes: BTreeMap<String, NoteIndex>,
    error: Option<String>,
    scanned_at: i64,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    resume_after: i64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    linux_legacy_import: Option<u64>,
    version: u32,
    generation: u64,
    workspaces: BTreeMap<String, WorkspaceIndex>,
    cursors: BTreeMap<String, i64>,
    notification_error: Option<String>,
    #[serde(default)]
    ios_coverage: BTreeMap<String, i64>, // v1 migration only
    #[serde(default)]
    ios_scheduled: BTreeMap<String, Coverage>,
    #[serde(default)]
    ios_refill: Option<(i64, i64)>,
    #[serde(default)]
    forgotten: std::collections::BTreeSet<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "until", rename_all = "snake_case")]
enum Coverage {
    Through(i64),
    Indefinite,
}
#[cfg(any(target_os = "ios", test))]
impl Coverage {
    fn through(&self, now: i64) -> i64 {
        match self {
            Self::Through(t) => (*t).min(now),
            Self::Indefinite => now,
        }
    }
}

#[derive(Clone)]
enum IndexUpdate {
    Register(String),
    Saved(String, String, String),
    Path(String, String, Option<String>),
}
static PENDING: Mutex<VecDeque<IndexUpdate>> = Mutex::new(VecDeque::new());
static FLUSH_LOCK: Mutex<()> = Mutex::new(());
struct CachedStore {
    stamp: Option<(std::time::SystemTime, u64)>,
    store: Store,
}
static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedStore>>> = OnceLock::new();
fn index_stamp(path: &std::path::Path) -> io::Result<Option<(std::time::SystemTime, u64)>> {
    match fs::metadata(path) {
        Ok(m) => Ok(Some((m.modified()?, m.len()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

static SOURCES: OnceLock<Mutex<HashMap<String, WorkspaceRef>>> = OnceLock::new();
static WAKE: AtomicBool = AtomicBool::new(true);
static IOS_SYNC: AtomicBool = AtomicBool::new(true);
static LAST_ERROR: OnceLock<Mutex<BTreeMap<&'static str, String>>> = OnceLock::new();
fn sources() -> &'static Mutex<HashMap<String, WorkspaceRef>> {
    SOURCES.get_or_init(Default::default)
}
fn report(area: &'static str, error: impl ToString) {
    if let Ok(mut last) = LAST_ERROR.get_or_init(Default::default).lock() {
        last.insert(area, error.to_string());
    }
}

pub fn data_dir() -> PathBuf {
    #[cfg(target_os = "ios")]
    {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join("Library/Application Support/Markerup/reminders")
    }
    #[cfg(not(target_os = "ios"))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
            })
            .join(if cfg!(all(debug_assertions, not(target_os = "linux"))) {
                "markerup-dev/reminders"
            } else {
                "markerup/reminders"
            })
    }
}
fn transaction_at<T>(
    directory: &std::path::Path,
    write: bool,
    f: impl FnOnce(&mut Store) -> io::Result<T>,
) -> io::Result<T> {
    fs::create_dir_all(directory)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("index.lock"))?;
    lock.lock_exclusive()?;
    let path = directory.join("index.json");
    let stamp = index_stamp(&path)?;
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| io::Error::other("Reminder cache lock failed"))?;
    // Invalidate before loading: a corrupt replacement must never leave stale
    // data available to a later transaction.
    if cache
        .get(directory)
        .is_none_or(|entry| entry.stamp != stamp)
    {
        cache.remove(directory);
        let mut store: Store = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Store::default(),
            Err(e) => return Err(e),
        };
        if store.version > 3 {
            return Err(io::Error::other(
                "Reminder index belongs to a newer Markerup version",
            ));
        }
        for (key, until) in std::mem::take(&mut store.ios_coverage) {
            store.ios_scheduled.insert(
                key,
                if until == i64::MAX {
                    Coverage::Indefinite
                } else {
                    Coverage::Through(until)
                },
            );
        }
        cache.insert(directory.to_path_buf(), CachedStore { stamp, store });
    }
    let cached = cache.get_mut(directory).unwrap();
    let store = &mut cached.store;
    let result = match f(store) {
        Ok(value) => value,
        Err(error) => {
            cache.remove(directory);
            return Err(error);
        }
    };
    if write {
        store.version = 3;
        store.generation = store.generation.wrapping_add(1);
        let persisted = (|| -> io::Result<()> {
            let mut file = fs::File::create(directory.join("index.tmp"))?;
            file.write_all(&serde_json::to_vec(&store).map_err(io::Error::other)?)?;
            file.sync_all()?;
            fs::rename(directory.join("index.tmp"), &path)?;
            fs::File::open(directory)?.sync_all()?;
            cached.stamp = index_stamp(&path)?;
            Ok(())
        })();
        if let Err(error) = persisted {
            cache.remove(directory);
            return Err(error);
        }
    }

    Ok(result)
}
fn transaction<T>(write: bool, f: impl FnOnce(&mut Store) -> io::Result<T>) -> io::Result<T> {
    transaction_at(&data_dir(), write, f)
}

pub fn register(workspace: WorkspaceRef) {
    let identity = workspace.identity();
    if let Ok(mut sources) = sources().lock() {
        sources.insert(identity.clone(), workspace);
    }
    PENDING
        .lock()
        .unwrap()
        .push_back(IndexUpdate::Register(identity));
    WAKE.store(true, Ordering::Release);
}

fn apply_updates(directory: &std::path::Path, updates: &VecDeque<IndexUpdate>) -> io::Result<()> {
    // Parse on the worker, coalescing successive saves of the same note. A path
    // mutation is an ordering barrier, so rename/delete cannot resurrect a note.
    let mut compact = Vec::new();
    let mut saves = BTreeMap::new();
    for update in updates {
        if let IndexUpdate::Saved(w, f, text) = update {
            saves.insert((w.clone(), f.clone()), text.clone());
        } else {
            compact.extend(
                std::mem::take(&mut saves)
                    .into_iter()
                    .map(|((w, f), text)| IndexUpdate::Saved(w, f, text)),
            );
            compact.push(update.clone());
        }
    }
    compact.extend(
        saves
            .into_iter()
            .map(|((w, f), text)| IndexUpdate::Saved(w, f, text)),
    );
    let parsed: Vec<_> = compact
        .iter()
        .map(|u| {
            if let IndexUpdate::Saved(_, _, text) = u {
                Some(schedule::parse(text))
            } else {
                None
            }
        })
        .collect();
    transaction_at(directory, true, |store| {
        for (update, parsed) in compact.iter().zip(parsed) {
            match update {
                IndexUpdate::Register(identity) => {
                    if store.forgotten.contains(identity) {
                        continue;
                    }
                    let index = store.workspaces.entry(identity.clone()).or_default();
                    #[cfg(target_os = "linux")]
                    if !identity.starts_with("smb:") {
                        index.local_path = Some(PathBuf::from(identity));
                    }
                    #[cfg(not(target_os = "linux"))]
                    let _ = index;
                }
                IndexUpdate::Saved(identity, file, _) => {
                    if store.forgotten.contains(identity) {
                        continue;
                    }
                    let index = store.workspaces.entry(identity.clone()).or_default();
                    index.generation = index.generation.wrapping_add(1);
                    index.notes.insert(
                        file.clone(),
                        NoteIndex {
                            revision: None,
                            checked_at: Utc::now().timestamp(),
                            parsed: parsed.unwrap(),
                        },
                    );
                }
                IndexUpdate::Path(identity, old, new) => {
                    if let Some(index) = store.workspaces.get_mut(identity) {
                        update_path(index, old, new.as_deref());
                    }
                }
            }
        }
        Ok(())
    })
}

fn flush_pending() -> io::Result<()> {
    let _guard = FLUSH_LOCK
        .lock()
        .map_err(|_| io::Error::other("Reminder save queue lock failed"))?;
    let mut batch = std::mem::take(&mut *PENDING.lock().unwrap());
    if batch.is_empty() {
        return Ok(());
    }
    if let Err(error) = apply_updates(&data_dir(), &batch) {
        let mut pending = PENDING.lock().unwrap();
        batch.append(&mut pending);
        *pending = batch;
        return Err(error);
    }
    IOS_SYNC.store(true, Ordering::Release);
    Ok(())
}

pub fn flush_on_exit() {
    if let Err(error) = flush_pending() {
        eprintln!("Could not flush reminder index on exit: {error}");
    }
}

pub fn refresh() {
    WAKE.store(true, Ordering::Release);
    IOS_SYNC.store(true, Ordering::Release);
}

fn update_path(index: &mut WorkspaceIndex, old: &str, new: Option<&str>) {
    let prefix = format!("{old}/");
    let affected = index
        .notes
        .keys()
        .filter(|id| id.as_str() == old || id.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    for id in affected {
        if let Some(note) = index.notes.remove(&id)
            && let Some(new) = new
        {
            index
                .notes
                .insert(format!("{new}{}", &id[old.len()..]), note);
        }
    }
    index.generation = index.generation.wrapping_add(1);
}

// Reconcile successful explicit mutations even if an unrelated note/provider
// is unreadable and a subsequent full directory reconciliation cannot finish.
pub fn path_changed(workspace: &WorkspaceRef, old: &str, new: Option<&str>) {
    PENDING.lock().unwrap().push_back(IndexUpdate::Path(
        workspace.identity(),
        old.to_string(),
        new.map(str::to_string),
    ));
    refresh();
}

// Only confirmed saved contents enter this queue. No parsing or disk I/O runs
// in the editor's save critical section.
pub fn saved(workspace: &WorkspaceRef, file: &str, contents: &str) {
    PENDING.lock().unwrap().push_back(IndexUpdate::Saved(
        workspace.identity(),
        file.to_string(),
        contents.to_string(),
    ));
}

fn scan(workspace: &WorkspaceRef, directory: &std::path::Path, force: bool) -> io::Result<()> {
    let identity = workspace.identity();
    let (generation, mut index) = transaction_at(directory, false, |store| {
        Ok((
            store.workspaces.get(&identity).map_or(0, |w| w.generation),
            store.workspaces.get(&identity).cloned().unwrap_or_default(),
        ))
    })?;
    if index.paused || transaction_at(directory, false, |s| Ok(s.forgotten.contains(&identity)))? {
        return Ok(());
    }
    let now = Utc::now().timestamp();
    let entries = workspace.entries()?;
    let mut errors = Vec::new();
    let revisions = workspace.reminder_revisions(&entries).unwrap_or_else(|e| {
        errors.push(format!("Metadata: {e}"));
        BTreeMap::new()
    });
    let mut next = BTreeMap::new();
    for entry in entries.into_iter().filter(|e| e.kind == EntryKind::File) {
        let revision = revisions.get(&entry.id).cloned();
        let old = index.notes.get(&entry.id);
        let unchanged = !force
            && revision.is_some()
            && old.is_some_and(|old| old.revision == revision && now - old.checked_at < 86400);
        let note = if unchanged {
            old.unwrap().clone()
        } else {
            // Enumeration succeeded; keep only the failed note's previous state.
            let text = match workspace.read(&entry.id) {
                Ok(text) => text,
                Err(error) => {
                    errors.push(format!("{}: {error}", entry.id));
                    if let Some(old) = old {
                        next.insert(entry.id, old.clone());
                    }
                    continue;
                }
            };
            NoteIndex {
                revision,
                checked_at: now,
                parsed: schedule::parse(&text),
            }
        };
        next.insert(entry.id, note);
    }
    #[cfg(target_os = "linux")]
    if !identity.starts_with("smb:") {
        index.local_path = Some(PathBuf::from(&identity));
    }
    index.notes = next;
    index.error = (!errors.is_empty()).then(|| errors.join("\n"));
    index.scanned_at = now;
    transaction_at(directory, true, |store| {
        // A concurrent save/index update wins over this older scan.
        if !store.forgotten.contains(&identity)
            && generation == store.workspaces.get(&identity).map_or(0, |w| w.generation)
        {
            index.generation = generation.wrapping_add(1);
            store.workspaces.insert(identity, index);
        } else {
            WAKE.store(true, Ordering::Release);
        }
        Ok(())
    })?;
    IOS_SYNC.store(true, Ordering::Release);
    Ok(())
}

fn scan_sources(force: bool) -> io::Result<()> {
    let workspaces = sources()
        .lock()
        .map(|s| s.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    for workspace in workspaces {
        if let Err(error) = scan(&workspace, &data_dir(), force) {
            let identity = workspace.identity();
            let message = error.to_string();
            transaction(true, |store| {
                if let Some(workspace) = store.workspaces.get_mut(&identity)
                    && !workspace.paused
                {
                    workspace.error = Some(message);
                }
                Ok(())
            })?;
        }
    }
    Ok(())
}

#[derive(Clone, Serialize)]
pub struct ReminderStatus {
    pub items: Vec<ReminderItem>,
    pub errors: Vec<String>,
    pub platform: platform::PlatformStatus,
    pub workspaces: Vec<WorkspaceStatus>,
}
#[derive(Clone, Serialize)]
pub struct WorkspaceStatus {
    pub identity: String,
    pub paused: bool,
    pub forgotten: bool,
}
#[derive(Clone, Serialize)]
pub struct ReminderItem {
    pub workspace: String,
    pub file: String,
    pub id: String,
    pub title: String,
    pub offset: usize,
    pub schedule: String,
    pub next: Option<String>,
    pub definition: schedule::Schedule,
    pub paused: bool,
    pub key: String,
}

pub fn status() -> Result<ReminderStatus, String> {
    flush_pending().map_err(|e| e.to_string())?;
    let mut result = transaction(false, |store| {
        let now = Utc::now();
        let mut items = Vec::new();
        let mut errors = Vec::new();
        for (identity, workspace) in &store.workspaces {
            if let Some(error) = &workspace.error { errors.push(format!("{identity}: {error}")); }
            let mut ids = std::collections::HashSet::new();
            for (file, note) in &workspace.notes {
                errors.extend(note.parsed.errors.iter().map(|e| format!("{file}: {e}")));
                for reminder in &note.parsed.reminders {
                    if !ids.insert(&reminder.id) { errors.push(format!("{file}: duplicate reminder ID {} in this workspace; only the first copy is scheduled", reminder.id)); }
                    items.push(ReminderItem { workspace: identity.clone(), file: file.clone(), id: reminder.id.clone(), title: reminder.title.clone(), offset: reminder.offset,
                        definition: reminder.schedule.clone(), paused: workspace.paused, key: delivery_key(identity, reminder), schedule: reminder.schedule.label(), next: reminder.schedule.next_after(now).map(|t| t.to_rfc3339()) });
                }
            }
        }
        if let Some(error) = &store.notification_error { errors.push(error.clone()); }
        let mut workspaces = store.workspaces.iter().map(|(identity,w)| WorkspaceStatus { identity: identity.clone(), paused: w.paused, forgotten: false }).collect::<Vec<_>>();
        workspaces.extend(store.forgotten.iter().map(|identity| WorkspaceStatus { identity: identity.clone(), paused: true, forgotten: true }));
        Ok(ReminderStatus { items, errors, platform: platform::status(), workspaces })
    }).map_err(|e| e.to_string())?;
    if let Ok(errors) = LAST_ERROR.get_or_init(Default::default).lock() {
        result.errors.extend(errors.values().cloned());
    }
    result.items.sort_by(|a, b| a.next.cmp(&b.next));
    Ok(result)
}

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
pub fn initialize(app: tauri::AppHandle) {
    let _ = APP.set(app);
    std::thread::Builder::new()
        .name("markerup-reminder-index".into())
        .spawn(|| {
            let mut last_scan = Instant::now() - std::time::Duration::from_secs(60);
            loop {
                if let Err(error) = flush_pending() {
                    report("index", error);
                } else if let Ok(mut errors) = LAST_ERROR.get_or_init(Default::default).lock() {
                    errors.remove("index");
                }
                if WAKE.swap(false, Ordering::AcqRel) || last_scan.elapsed().as_secs() >= 60 {
                    if let Err(error) = scan_sources(false) {
                        report("scan", error);
                    } else if let Ok(mut errors) = LAST_ERROR.get_or_init(Default::default).lock() {
                        errors.remove("scan");
                    }
                    last_scan = Instant::now();
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        })
        .expect("could not start reminder index worker");
    std::thread::Builder::new()
        .name("markerup-reminder-scheduler".into())
        .spawn(|| {
            loop {
                if let Err(error) = platform::synchronize() {
                    report("scheduler", error);
                } else if let Ok(mut errors) = LAST_ERROR.get_or_init(Default::default).lock() {
                    errors.remove("scheduler");
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        })
        .expect("could not start reminder scheduler");
}

#[cfg(target_os = "ios")]
pub fn flush_notifications() -> Result<(), String> {
    flush_pending().map_err(|e| e.to_string())?;
    IOS_SYNC.store(true, Ordering::Release);
    platform::synchronize().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reminder_take_notification() -> Result<Option<String>, String> {
    crate::tauri_backend::run_blocking(platform::take_notification).await
}

#[derive(Clone)]
struct Delivery {
    key: String,
    workspace: String,
    title: String,
    body: String,
    due: DateTime<Utc>,
}
fn deliveries(store: &Store, now: DateTime<Utc>) -> Vec<Delivery> {
    let mut result = Vec::new();
    for (workspace, index) in &store.workspaces {
        if index.paused {
            continue;
        }
        let mut ids = std::collections::HashSet::new();
        for (file, note) in &index.notes {
            for reminder in &note.parsed.reminders {
                // Same ID in copied notes must not produce two notifications.
                if !ids.insert(reminder.id.clone()) {
                    continue;
                }
                let key = delivery_key(workspace, reminder);
                let cursor = store
                    .cursors
                    .get(&key)
                    .and_then(|t| DateTime::from_timestamp(*t, 0))
                    .unwrap_or_else(|| {
                        if reminder.schedule.repeat == Repeat::Once {
                            DateTime::from_timestamp(0, 0).unwrap()
                        } else {
                            now - Duration::days(1)
                        }
                    });
                let cursor =
                    cursor.max(DateTime::from_timestamp(index.resume_after, 0).unwrap_or(cursor));
                if let Some(due) = reminder.schedule.next_after(cursor)
                    && due <= now
                {
                    result.push(Delivery {
                        key,
                        workspace: workspace.clone(),
                        title: reminder.title.clone(),
                        body: format!("{file} · {}", reminder.schedule.label()),
                        due,
                    });
                }
            }
        }
    }
    result
}
fn delivery_key(workspace: &str, reminder: &Reminder) -> String {
    format!(
        "{:016x}-{}-{:016x}",
        schedule::stable_hash(workspace),
        reminder.id,
        schedule::stable_hash(&serde_json::to_string(&reminder.schedule).unwrap_or_default())
    )
}

#[tauri::command]
pub async fn reminder_status() -> Result<ReminderStatus, String> {
    crate::tauri_backend::run_blocking(status).await
}
#[tauri::command]
pub async fn reminder_permissions() -> Result<String, String> {
    crate::tauri_backend::run_blocking(platform::request_permission).await
}
#[tauri::command]
pub async fn reminder_rescan() -> Result<(), String> {
    crate::tauri_backend::run_blocking(|| {
        flush_pending().map_err(|e| e.to_string())?;
        scan_sources(true).map_err(|e| e.to_string())?;
        if let Ok(mut error) = LAST_ERROR.get_or_init(Default::default).lock() {
            error.remove("index");
            error.remove("scan");
        }
        platform::synchronize().map_err(|e| e.to_string())
    })
    .await
}
#[tauri::command]
pub fn reminder_validate(spec: String) -> Result<String, String> {
    let (schedule, _) = schedule::parse_spec(&spec)?;
    schedule
        .next_after(Utc::now())
        .map(|t| t.to_rfc3339())
        .ok_or("This schedule has no future occurrence".into())
}

#[cfg(target_os = "linux")]
pub fn run_service() -> io::Result<()> {
    platform::run_service()
}

#[tauri::command]
pub async fn reminder_workspace_action(identity: String, action: String) -> Result<(), String> {
    crate::tauri_backend::run_blocking(move || {
        flush_pending().map_err(|e| e.to_string())?;
        transaction(true, |store| {
            match action.as_str() {
                "forget" => {
                    store.workspaces.remove(&identity);
                    store.forgotten.insert(identity.clone());
                }
                "resume" => {
                    store.forgotten.remove(&identity);
                    let w = store.workspaces.entry(identity.clone()).or_default();
                    w.resume_after = Utc::now().timestamp();
                    w.paused = false;
                    w.generation += 1;
                    #[cfg(target_os = "linux")]
                    if !identity.starts_with("smb:") {
                        w.local_path = Some(PathBuf::from(&identity));
                    }
                }
                "pause" => {
                    let w = store
                        .workspaces
                        .get_mut(&identity)
                        .ok_or_else(|| io::Error::other("Workspace not found"))?;
                    w.paused = true;
                    w.generation += 1;
                }
                _ => return Err(io::Error::other("Unknown reminder action")),
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
        refresh();
        platform::synchronize().map_err(|e| e.to_string())
    })
    .await
}

#[derive(Clone, Serialize)]
pub struct ReminderTarget {
    pub workspace: String,
    pub file: String,
    pub id: String,
}
pub fn target(key: &str) -> Result<Option<ReminderTarget>, String> {
    transaction(false, |s| {
        for (workspace, index) in &s.workspaces {
            for (file, note) in &index.notes {
                for reminder in &note.parsed.reminders {
                    if delivery_key(workspace, reminder) == key {
                        return Ok(Some(ReminderTarget {
                            workspace: workspace.clone(),
                            file: file.clone(),
                            id: reminder.id.clone(),
                        }));
                    }
                }
            }
        }
        Ok(None)
    })
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn reminder_target(key: String) -> Result<Option<ReminderTarget>, String> {
    crate::tauri_backend::run_blocking(move || target(&key)).await
}
#[tauri::command]
pub fn reminder_definition(source: String, id: String) -> Result<Reminder, String> {
    schedule::parse(&source)
        .reminders
        .into_iter()
        .find(|r| r.id == id)
        .ok_or("Reminder changed or no longer exists. Refresh the list.".into())
}

pub fn queue_navigation(args: &[String]) {
    platform::queue_navigation(args);
}

#[tauri::command]
pub fn reminder_definitions(source: String) -> Vec<Reminder> {
    schedule::parse(&source).reminders
}

#[cfg(target_os = "ios")]
fn publish_health(status: &platform::PlatformStatus) {
    use tauri::Emitter;
    if let Some(app) = APP.get() {
        let _ = app.emit("reminder-health", status);
    }
}

#[cfg(target_os = "linux")]
pub fn setup_service(remove: bool) -> io::Result<()> {
    platform::setup_service(remove)
}
