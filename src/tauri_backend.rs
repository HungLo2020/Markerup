use crate::markdown::{
    PreviewDocument, preview_document as parse_preview_document, toggle_task_at_offset,
};
use crate::navigation::NavigationState;
use crate::persistence::{
    SavedFavorite, SavedSmbConfig, SavedWorkspace, clear_session, load_session, save_session,
};
use crate::smb_workspace::{SmbConnectionConfig, SmbWorkspace};
#[cfg(not(target_os = "ios"))]
use crate::workspace::LocalWorkspace;
use crate::workspace::{
    EntryId, EntryKind, LinkTarget, Workspace, WorkspaceEntry, WorkspaceRef, WorkspaceSlot,
};
use base64::Engine;
use merman::MermaidConfig;
use merman::render::HeadlessRenderer;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
#[cfg(not(target_os = "ios"))]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use tauri::{AppHandle, Manager};

const PRIVACY_POLICY_URL: &str = "https://hunglo2020.github.io/Markerup/privacy-policy/";

#[derive(Default)]
struct BackendInner {
    workspace: WorkspaceSlot,
    workspace_revision: u64,
    entries: Vec<WorkspaceEntry>,
    bookmark: Option<Vec<u8>>,
    favorites: Vec<SavedFavorite>,
    active_favorite: Option<usize>,
    current_file: Option<EntryId>,
    disk_text: String,
    external_conflict: bool,
    navigation: NavigationState,
    /// A saved favorite is being reopened in the background.
    restoring: bool,
    /// The workspace revision the startup restore installed, if any.
    restored_revision: Option<u64>,
}

/// The sole owner of canonical workspace state. The web UI keeps only the
/// active editor buffer; every filesystem/SMB mutation is performed here.
#[derive(Default)]
pub struct MarkerupBackend {
    inner: Mutex<BackendInner>,
    asset_cache: Mutex<AssetCache>,
    restore_finished: Condvar,
    search_generation: AtomicU64,
}

/// The active favorite from the saved session, loaded at launch and reopened
/// in the background so a slow SMB server or Files provider cannot delay the
/// first window.
pub struct PendingRestore {
    favorite: SavedFavorite,
    current_file: Option<EntryId>,
    revision: u64,
}

type ReopenedWorkspace = (WorkspaceSlot, Option<Vec<u8>>, Vec<WorkspaceEntry>);

struct SaveRequest {
    workspace: WorkspaceRef,
    file: EntryId,
    baseline: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AssetCacheKey {
    workspace: String,
    id: EntryId,
}

struct CachedAsset {
    data_url: String,
    size: usize,
    last_used: u64,
}

#[derive(Default)]
struct AssetCache {
    entries: HashMap<AssetCacheKey, CachedAsset>,
    used_bytes: usize,
    clock: u64,
}

const MAX_CACHED_ASSETS: usize = 64;
const MAX_CACHED_ASSET_BYTES: usize = 32 * 1024 * 1024;

impl AssetCache {
    fn get(&mut self, key: &AssetCacheKey) -> Option<String> {
        let asset = self.entries.get_mut(key)?;
        self.clock = self.clock.wrapping_add(1);
        asset.last_used = self.clock;
        Some(asset.data_url.clone())
    }

    fn insert(&mut self, key: AssetCacheKey, data_url: String) {
        let size = data_url.len();
        if size > MAX_CACHED_ASSET_BYTES {
            return;
        }
        if let Some(previous) = self.entries.remove(&key) {
            self.used_bytes = self.used_bytes.saturating_sub(previous.size);
        }
        self.clock = self.clock.wrapping_add(1);
        self.used_bytes += size;
        self.entries.insert(
            key,
            CachedAsset {
                data_url,
                size,
                last_used: self.clock,
            },
        );
        while self.entries.len() > MAX_CACHED_ASSETS || self.used_bytes > MAX_CACHED_ASSET_BYTES {
            let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, asset)| asset.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(oldest) = self.entries.remove(&oldest_key) {
                self.used_bytes = self.used_bytes.saturating_sub(oldest.size);
            }
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.used_bytes = 0;
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub workspace_open: bool,
    pub workspace_path: String,
    pub workspace_is_smb: bool,
    pub workspace_favorited: bool,
    pub favorites: Vec<FavoritePayload>,
    pub entries: Vec<WorkspaceEntry>,
    pub current_file: Option<EntryId>,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub external_conflict: bool,
    pub workspace_restoring: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoritePayload {
    pub index: usize,
    pub label: String,
    pub workspace_is_smb: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotePayload {
    pub id: EntryId,
    pub contents: String,
    pub snapshot: WorkspaceSnapshot,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmbConnectRequest {
    pub server: String,
    pub share: String,
    pub username: String,
    pub password: String,
    pub remote_path: String,
}

impl MarkerupBackend {
    fn cached_asset(&self, key: &AssetCacheKey) -> Option<String> {
        self.asset_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
    }

    fn cache_asset(&self, key: AssetCacheKey, data_url: String) {
        self.asset_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key, data_url);
    }

    fn clear_asset_cache(&self) {
        self.asset_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    fn locked(&self) -> Result<std::sync::MutexGuard<'_, BackendInner>, String> {
        match self.inner.lock() {
            Ok(inner) => Ok(inner),
            Err(poisoned) => {
                // A Rust panic in a command must not permanently prevent the
                // user from opening a workspace. BackendInner has no partial
                // transaction invariants: each command either updates its
                // fields directly or reports an ordinary Result error. Keep
                // the data and let the next command replace/reconcile it.
                eprintln!("Markerup recovered workspace state after a prior command panic");
                Ok(poisoned.into_inner())
            }
        }
    }

    fn snapshot(inner: &BackendInner) -> WorkspaceSnapshot {
        let workspace_open = inner.workspace.is_open();
        WorkspaceSnapshot {
            workspace_open,
            workspace_path: inner.workspace.root_display(),
            workspace_is_smb: matches!(inner.workspace, WorkspaceSlot::Smb(_)),
            workspace_favorited: inner.active_favorite.is_some(),
            favorites: inner
                .favorites
                .iter()
                .enumerate()
                .map(|(index, favorite)| FavoritePayload {
                    index,
                    label: Self::favorite_label(favorite),
                    workspace_is_smb: matches!(favorite.workspace, SavedWorkspace::Smb(_)),
                })
                .collect(),
            entries: inner.entries.clone(),
            current_file: inner.current_file.clone(),
            can_go_back: inner.navigation.can_go_back(),
            can_go_forward: inner.navigation.can_go_forward(),
            external_conflict: inner.external_conflict,
            workspace_restoring: inner.restoring,
        }
    }

    fn refresh_entries(&self) -> Result<(), String> {
        for _ in 0..2 {
            // Workspace providers perform recursive/local/SMB I/O here. Keep
            // only a cheap handle snapshot under the state mutex.
            let (workspace, revision) = {
                let inner = self.locked()?;
                (inner.workspace.shared_clone(), inner.workspace_revision)
            };
            let entries = match workspace {
                Some(workspace) => workspace.entries().map_err(|error| error.to_string())?,
                None => Vec::new(),
            };
            let mut inner = self.locked()?;
            // A workspace mutation or switch may have happened while the
            // provider was scanning. Never publish an obsolete scan.
            if inner.workspace_revision != revision {
                continue;
            }
            inner.entries = entries;
            return Ok(());
        }
        Err("Workspace changed while refreshing; please try again".to_string())
    }

    fn mark_workspace_changed(inner: &mut BackendInner) {
        inner.workspace_revision = inner.workspace_revision.wrapping_add(1);
    }

    fn reconcile_disk_change(
        inner: &mut BackendInner,
        disk: &str,
        editor_has_unsaved_changes: bool,
    ) {
        if disk != inner.disk_text {
            // Preserve the original save baseline until the frontend loads the
            // new note. This also protects edits made during a refresh scan.
            inner.external_conflict = true;
        } else if !editor_has_unsaved_changes {
            inner.external_conflict = false;
        }
    }

    fn persist(inner: &BackendInner) {
        if inner.favorites.is_empty() {
            let _ = clear_session();
            return;
        }
        let _ = save_session(
            &inner.favorites,
            inner.active_favorite,
            inner
                .active_favorite
                .is_some()
                .then_some(inner.current_file.as_deref())
                .flatten(),
        );
    }

    fn install_workspace(
        inner: &mut BackendInner,
        workspace: WorkspaceSlot,
        bookmark: Option<Vec<u8>>,
    ) {
        inner.active_favorite = Self::favorite_index_for(&inner.favorites, &workspace, &bookmark);
        inner.workspace = workspace;
        Self::mark_workspace_changed(inner);
        inner.entries.clear();
        inner.bookmark = bookmark;
        inner.current_file = None;
        inner.disk_text.clear();
        inner.external_conflict = false;
        inner.navigation = NavigationState::default();
        Self::persist(inner);
    }

    fn favorite_label(favorite: &SavedFavorite) -> String {
        match &favorite.workspace {
            SavedWorkspace::Local { path, .. } => path.to_string_lossy().into_owned(),
            SavedWorkspace::Smb(config) => {
                let folder = config.remote_path.trim_matches('/');
                if folder.is_empty() {
                    format!("smb://{}/{}", config.server, config.share)
                } else {
                    format!("smb://{}/{}/{}", config.server, config.share, folder)
                }
            }
        }
    }

    fn workspace_favorite(
        workspace: &WorkspaceSlot,
        bookmark: Option<&[u8]>,
    ) -> Option<SavedFavorite> {
        if let Some(config) = workspace.smb_config() {
            return Some(SavedFavorite {
                workspace: SavedWorkspace::Smb(SavedSmbConfig {
                    server: config.server,
                    share: config.share,
                    username: config.username,
                    remote_path: config.remote_path,
                }),
            });
        }
        workspace.root_path().map(|path| SavedFavorite {
            workspace: SavedWorkspace::Local {
                path: path.to_path_buf(),
                bookmark: bookmark.map(ToOwned::to_owned),
            },
        })
    }

    fn favorite_index_for(
        favorites: &[SavedFavorite],
        workspace: &WorkspaceSlot,
        bookmark: &Option<Vec<u8>>,
    ) -> Option<usize> {
        let current = Self::workspace_favorite(workspace, bookmark.as_deref())?;
        favorites.iter().position(|favorite| favorite == &current)
    }

    fn open_note_locked(
        inner: &mut BackendInner,
        id: EntryId,
        record_navigation: bool,
    ) -> Result<NotePayload, String> {
        let contents = inner
            .workspace
            .read(&id)
            .map_err(|error| error.to_string())?;
        if record_navigation && inner.current_file.as_deref() != Some(id.as_str()) {
            inner.navigation.visit(inner.current_file.as_deref());
        }
        inner.current_file = Some(id.clone());
        inner.disk_text = contents.clone();
        inner.external_conflict = false;
        Self::persist(inner);
        let snapshot = Self::snapshot(inner);
        Ok(NotePayload {
            id,
            contents,
            snapshot,
        })
    }

    fn current_file(inner: &BackendInner) -> Result<EntryId, String> {
        inner
            .current_file
            .clone()
            .ok_or_else(|| "No note is selected".to_string())
    }

    fn begin_save(&self, force: bool) -> Result<SaveRequest, String> {
        let inner = self.locked()?;
        if inner.external_conflict && !force {
            return Err("External change conflict".to_string());
        }
        Ok(SaveRequest {
            workspace: inner
                .workspace
                .shared_clone()
                .ok_or_else(|| "No workspace is open".to_string())?,
            file: Self::current_file(&inner)?,
            baseline: inner.disk_text.clone(),
        })
    }

    fn mark_conflict_if_current(&self, request: &SaveRequest) {
        if let Ok(mut inner) = self.locked()
            && inner.current_file.as_deref() == Some(request.file.as_str())
            && inner.disk_text == request.baseline
        {
            inner.external_conflict = true;
        }
    }

    fn complete_save(
        &self,
        request: &SaveRequest,
        contents: &str,
    ) -> Result<WorkspaceSnapshot, String> {
        let mut inner = self.locked()?;
        if inner.current_file.as_deref() != Some(request.file.as_str())
            || inner.disk_text != request.baseline
        {
            return Err("The active note changed while its save was in progress; reload before editing further.".to_string());
        }
        inner.disk_text = contents.to_string();
        inner.external_conflict = false;
        Self::persist(&inner);
        Ok(Self::snapshot(&inner))
    }

    fn install_scanned(
        &self,
        workspace: WorkspaceSlot,
        bookmark: Option<Vec<u8>>,
        entries: Vec<WorkspaceEntry>,
    ) -> Result<WorkspaceSnapshot, String> {
        let mut inner = self.locked()?;
        Self::install_workspace(&mut inner, workspace, bookmark);
        inner.entries = entries;
        let snapshot = Self::snapshot(&inner);
        drop(inner);
        self.clear_asset_cache();
        Ok(snapshot)
    }

    /// Load saved favorites before the window opens. The session file is a
    /// small local file, and loading it up front means the UI can list
    /// favorites immediately and a favorite toggle can never overwrite them.
    /// Reopening the active favorite can require SMB or a slow Files
    /// provider, so it is returned for [`Self::finish_restore`] to perform
    /// off the startup path.
    pub fn load_saved_session(&self) -> Option<PendingRestore> {
        let session = load_session()?;
        let mut inner = self.locked().ok()?;
        inner.favorites = session.favorites;
        let favorite = inner.favorites.get(session.active_favorite?)?.clone();
        inner.restoring = true;
        Some(PendingRestore {
            favorite,
            current_file: session.current_file,
            revision: inner.workspace_revision,
        })
    }

    /// Reopen the favorite recorded by [`Self::load_saved_session`]. This
    /// performs workspace I/O and must run on a background thread.
    pub fn finish_restore(&self, pending: PendingRestore) {
        let reopened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Self::reopen_favorite(pending.favorite)
        }))
        .ok()
        .flatten();
        let Ok(mut inner) = self.locked() else {
            return;
        };
        inner.restoring = false;
        // A workspace the user chose while the favorite was reopening wins.
        if let Some((workspace, bookmark, entries)) = reopened
            && inner.workspace_revision == pending.revision
        {
            Self::install_workspace(&mut inner, workspace, bookmark);
            inner.entries = entries;
            if let Some(id) = pending.current_file {
                let _ = Self::open_note_locked(&mut inner, id, false);
            }
            inner.restored_revision = Some(inner.workspace_revision);
        }
        drop(inner);
        self.clear_asset_cache();
        self.restore_finished.notify_all();
    }

    #[cfg(not(target_os = "ios"))]
    fn reopen_favorite(favorite: SavedFavorite) -> Option<ReopenedWorkspace> {
        match favorite.workspace {
            SavedWorkspace::Local { path, .. } => {
                LocalWorkspace::open(path).ok().and_then(|workspace| {
                    let entries = workspace.entries().ok()?;
                    Some((WorkspaceSlot::local(workspace), None, entries))
                })
            }
            SavedWorkspace::Smb(_) => None,
        }
    }

    #[cfg(target_os = "ios")]
    fn reopen_favorite(favorite: SavedFavorite) -> Option<ReopenedWorkspace> {
        match favorite.workspace {
            SavedWorkspace::Smb(smb) => crate::ios_bridge::load_smb_password(
                &smb_keychain_account(&smb),
            )
            .and_then(|password| {
                let workspace = SmbWorkspace::connect(SmbConnectionConfig {
                    server: smb.server,
                    share: smb.share,
                    username: smb.username,
                    password,
                    remote_path: smb.remote_path,
                })
                .ok()?;
                let entries = workspace.entries().ok()?;
                Some((WorkspaceSlot::smb(workspace), None, entries))
            }),
            SavedWorkspace::Local {
                bookmark: Some(bookmark),
                ..
            } => crate::ios_bridge::resolve_bookmark(&bookmark)
                .ok()
                .and_then(|selection| {
                    let workspace = crate::ios_workspace::IosWorkspace::open(selection).ok()?;
                    let entries = workspace.entries().ok()?;
                    Some((WorkspaceSlot::ios(workspace), Some(bookmark), entries))
                }),
            SavedWorkspace::Local { bookmark: None, .. } => None,
        }
    }

    /// Wait for a startup restore to finish. Returns the restored workspace
    /// only when it is still the one installed.
    fn restored_workspace(&self) -> Result<Option<WorkspaceSnapshot>, String> {
        let mut inner = self.locked()?;
        while inner.restoring {
            inner = self
                .restore_finished
                .wait(inner)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        Ok((inner.restored_revision == Some(inner.workspace_revision))
            .then(|| Self::snapshot(&inner)))
    }

    fn workspace_snapshot(&self) -> Result<WorkspaceSnapshot, String> {
        let inner = self.locked()?;
        Ok(Self::snapshot(&inner))
    }

    #[cfg(not(target_os = "ios"))]
    fn open_local_workspace(&self, path: PathBuf) -> Result<WorkspaceSnapshot, String> {
        let workspace = LocalWorkspace::open(path).map_err(|error| error.to_string())?;
        let entries = workspace.entries().map_err(|error| error.to_string())?;
        self.install_scanned(WorkspaceSlot::local(workspace), None, entries)
    }

    #[cfg(target_os = "ios")]
    fn install_ios_selection(
        &self,
        selection: crate::ios_bridge::WorkspaceSelection,
    ) -> Result<WorkspaceSnapshot, String> {
        let bookmark = selection.bookmark.clone();
        let workspace = crate::ios_workspace::IosWorkspace::open(selection)
            .map_err(|error| error.to_string())?;
        let entries = workspace.entries().map_err(|error| error.to_string())?;
        self.install_scanned(WorkspaceSlot::ios(workspace), Some(bookmark), entries)
    }

    fn connect_smb(&self, request: SmbConnectRequest) -> Result<WorkspaceSnapshot, String> {
        let config = SmbConnectionConfig {
            server: request.server,
            share: request.share,
            username: request.username,
            password: request.password,
            remote_path: request.remote_path,
        };
        let workspace = SmbWorkspace::connect(config).map_err(|error| error.to_string())?;
        let entries = workspace.entries().map_err(|error| error.to_string())?;
        self.install_scanned(WorkspaceSlot::smb(workspace), None, entries)
    }

    fn open_note(&self, id: EntryId) -> Result<NotePayload, String> {
        let mut inner = self.locked()?;
        Self::open_note_locked(&mut inner, id, true)
    }

    fn save_note(&self, contents: &str, force: bool) -> Result<WorkspaceSnapshot, String> {
        let request = self.begin_save(force)?;
        let current_disk = request
            .workspace
            .read(&request.file)
            .map_err(|error| error.to_string())?;
        if !force && current_disk != request.baseline {
            self.mark_conflict_if_current(&request);
            return Err("External change conflict".to_string());
        }
        if let Err(error) = request.workspace.write(&request.file, contents) {
            if error.kind() == std::io::ErrorKind::Interrupted {
                return match request.workspace.read(&request.file) {
                    Ok(remote) if remote == contents => self.complete_save(&request, contents),
                    Ok(_) => {
                        self.mark_conflict_if_current(&request);
                        Err("Save outcome unknown — the SMB server could have applied the write or another client changed the note. Reload before retrying.".to_string())
                    }
                    Err(verification_error) => Err(format!(
                        "Save outcome unknown — SMB write failed and Markerup could not verify the remote note: {verification_error}. Reload before retrying."
                    )),
                };
            }
            return Err(error.to_string());
        }
        let verified_disk = request
            .workspace
            .read(&request.file)
            .map_err(|error| format!("Save completed but Markerup could not verify it: {error}"))?;
        if verified_disk != contents {
            self.mark_conflict_if_current(&request);
            return Err("External change conflict — the note changed while Markerup was saving. Reload before retrying.".to_string());
        }
        self.complete_save(&request, contents)
    }

    fn reload_note(&self) -> Result<NotePayload, String> {
        let mut inner = self.locked()?;
        let id = Self::current_file(&inner)?;
        Self::open_note_locked(&mut inner, id, false)
    }

    fn refresh_workspace(
        &self,
        editor_has_unsaved_changes: bool,
    ) -> Result<WorkspaceSnapshot, String> {
        let (workspace, revision, current_file, baseline) = {
            let inner = self.locked()?;
            (
                inner.workspace.shared_clone(),
                inner.workspace_revision,
                inner.current_file.clone(),
                inner.disk_text.clone(),
            )
        };
        let disk = match (&workspace, &current_file) {
            (Some(workspace), Some(file)) => {
                Some(workspace.read(file).map_err(|error| error.to_string())?)
            }
            _ => None,
        };
        let entries = match workspace {
            Some(workspace) => workspace.entries().map_err(|error| error.to_string())?,
            None => Vec::new(),
        };
        let mut inner = self.locked()?;
        if inner.workspace_revision != revision
            || inner.current_file != current_file
            || inner.disk_text != baseline
        {
            return Ok(Self::snapshot(&inner));
        }
        if let Some(disk) = disk {
            Self::reconcile_disk_change(&mut inner, &disk, editor_has_unsaved_changes);
        }
        inner.entries = entries;
        let snapshot = Self::snapshot(&inner);
        drop(inner);
        self.clear_asset_cache();
        Ok(snapshot)
    }

    /// Search the notes in the current tree. Returns `None` when a newer
    /// search superseded this one before it finished; the caller discards it.
    fn search_workspace(&self, query: &str) -> Result<Option<Vec<EntryId>>, String> {
        let generation = self.search_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(Some(Vec::new()));
        }
        // Search the already-scanned tree rather than rescanning the
        // workspace for every query; results are shown within that tree.
        let (workspace, notes) = {
            let inner = self.locked()?;
            let workspace = inner
                .workspace
                .shared_clone()
                .ok_or_else(|| "No workspace is open".to_string())?;
            let notes: Vec<EntryId> = inner
                .entries
                .iter()
                .filter(|entry| entry.kind == EntryKind::File)
                .map(|entry| entry.id.clone())
                .collect();
            (workspace, notes)
        };
        let mut results = Vec::new();
        for id in notes {
            // Stop reading notes as soon as a newer query arrives.
            if self.search_generation.load(Ordering::SeqCst) != generation {
                return Ok(None);
            }
            if id.to_lowercase().contains(&query)
                || workspace
                    .read(&id)
                    .is_ok_and(|text| text.to_lowercase().contains(&query))
            {
                results.push(id);
            }
        }
        Ok(Some(results))
    }

    fn workspace_assets(&self) -> Result<Vec<WorkspaceEntry>, String> {
        let workspace = self
            .locked()?
            .workspace
            .shared_clone()
            .ok_or_else(|| "No workspace is open".to_string())?;
        workspace.asset_entries().map_err(|error| error.to_string())
    }

    fn create_note(&self, parent: &str, name: &str) -> Result<NotePayload, String> {
        let id = {
            let mut inner = self.locked()?;
            let id = inner
                .workspace
                .create_note(parent, name)
                .map_err(|error| error.to_string())?;
            Self::mark_workspace_changed(&mut inner);
            id
        };
        self.clear_asset_cache();
        self.refresh_entries()?;
        let mut inner = self.locked()?;
        Self::open_note_locked(&mut inner, id, true)
    }

    fn create_folder(&self, parent: &str, name: &str) -> Result<WorkspaceSnapshot, String> {
        {
            let mut inner = self.locked()?;
            inner
                .workspace
                .create_directory(parent, name)
                .map_err(|error| error.to_string())?;
            Self::mark_workspace_changed(&mut inner);
        }
        self.clear_asset_cache();
        self.refresh_entries()?;
        self.workspace_snapshot()
    }

    fn rename_entry(&self, id: &str, name: &str) -> Result<WorkspaceSnapshot, String> {
        {
            let mut inner = self.locked()?;
            let new_id = inner
                .workspace
                .rename(id, name)
                .map_err(|error| error.to_string())?;
            inner.navigation.rebase(id, &new_id);
            if inner.current_file.as_deref() == Some(id) {
                inner.current_file = Some(new_id);
            }
            Self::mark_workspace_changed(&mut inner);
            Self::persist(&inner);
        }
        self.clear_asset_cache();
        self.refresh_entries()?;
        self.workspace_snapshot()
    }

    fn move_entry(&self, id: &str, destination_parent: &str) -> Result<WorkspaceSnapshot, String> {
        {
            let mut inner = self.locked()?;
            let new_id = inner
                .workspace
                .move_entry(id, destination_parent)
                .map_err(|error| error.to_string())?;
            inner.navigation.rebase(id, &new_id);
            if inner.current_file.as_deref() == Some(id) {
                inner.current_file = Some(new_id.clone());
            } else if inner
                .current_file
                .as_deref()
                .is_some_and(|file| file.starts_with(&format!("{id}/")))
            {
                inner.current_file = inner
                    .current_file
                    .take()
                    .map(|file| format!("{}{}", new_id, &file[id.len()..]));
            }
            Self::mark_workspace_changed(&mut inner);
            Self::persist(&inner);
        }
        self.clear_asset_cache();
        self.refresh_entries()?;
        self.workspace_snapshot()
    }

    fn delete_entry(&self, id: &str) -> Result<WorkspaceSnapshot, String> {
        {
            let mut inner = self.locked()?;
            inner
                .workspace
                .delete(id)
                .map_err(|error| error.to_string())?;
            inner.navigation.remove(id);
            if inner.current_file.as_deref() == Some(id)
                || inner
                    .current_file
                    .as_deref()
                    .is_some_and(|file| file.starts_with(&format!("{id}/")))
            {
                inner.current_file = None;
                inner.disk_text.clear();
            }
            Self::mark_workspace_changed(&mut inner);
            Self::persist(&inner);
        }
        self.clear_asset_cache();
        self.refresh_entries()?;
        self.workspace_snapshot()
    }

    fn navigate_markdown_link(&self, link: &str) -> Result<NotePayload, String> {
        let mut inner = self.locked()?;
        let current = Self::current_file(&inner)?;
        let LinkTarget { id, .. } = inner
            .workspace
            .resolve_markdown_link(&current, link)
            .ok_or_else(|| {
                "Link does not point to a Markdown note in this workspace".to_string()
            })?;
        Self::open_note_locked(&mut inner, id, true)
    }

    fn step_history(&self, forward: bool) -> Result<Option<NotePayload>, String> {
        let mut inner = self.locked()?;
        let current = inner.current_file.clone();
        let target = if forward {
            inner.navigation.go_forward(current.as_deref())
        } else {
            inner.navigation.go_back(current.as_deref())
        };
        let Some(target) = target else {
            return Ok(None);
        };
        Self::open_note_locked(&mut inner, target, false).map(Some)
    }

    fn set_workspace_favorite(&self, favorited: bool) -> Result<WorkspaceSnapshot, String> {
        let mut inner = self.locked()?;
        let current = Self::workspace_favorite(&inner.workspace, inner.bookmark.as_deref())
            .ok_or_else(|| "Select a workspace before adding it to Favorites".to_string())?;
        if favorited {
            if let Some(index) = inner
                .favorites
                .iter()
                .position(|favorite| favorite == &current)
            {
                inner.active_favorite = Some(index);
                Self::persist(&inner);
                return Ok(Self::snapshot(&inner));
            }
            if matches!(inner.workspace, WorkspaceSlot::Smb(_)) {
                #[cfg(target_os = "ios")]
                let config = inner
                    .workspace
                    .smb_config()
                    .ok_or_else(|| "SMB workspace configuration is unavailable".to_string())?;
                #[cfg(target_os = "ios")]
                crate::ios_bridge::save_smb_password(&config.keychain_account(), &config.password)?;
                #[cfg(not(target_os = "ios"))]
                return Err("SMB credentials remain session-only on Linux".to_string());
            }
            inner.favorites.push(current);
            inner.active_favorite = Some(inner.favorites.len() - 1);
        } else if let Some(index) = inner.active_favorite {
            let removed = inner.favorites.remove(index);
            #[cfg(target_os = "ios")]
            if let SavedWorkspace::Smb(config) = removed.workspace {
                crate::ios_bridge::delete_smb_password(&smb_keychain_account(&config));
            }
            #[cfg(not(target_os = "ios"))]
            let _ = removed;
            inner.active_favorite = None;
        }
        Self::persist(&inner);
        Ok(Self::snapshot(&inner))
    }

    fn favorite(&self, index: usize) -> Result<SavedFavorite, String> {
        self.locked()?
            .favorites
            .get(index)
            .cloned()
            .ok_or_else(|| "Favorite workspace no longer exists".to_string())
    }

    #[cfg(not(target_os = "ios"))]
    fn open_favorite_workspace(&self, index: usize) -> Result<WorkspaceSnapshot, String> {
        let SavedWorkspace::Local { path, .. } = self.favorite(index)?.workspace else {
            return Err("SMB favorites require iOS Keychain-backed credentials".to_string());
        };
        self.open_local_workspace(path)
    }

    #[cfg(target_os = "ios")]
    fn open_favorite_workspace(&self, index: usize) -> Result<WorkspaceSnapshot, String> {
        let (workspace, bookmark) = match self.favorite(index)?.workspace {
            SavedWorkspace::Local {
                bookmark: Some(bookmark),
                ..
            } => {
                let selection = crate::ios_bridge::resolve_bookmark(&bookmark)?;
                let workspace = crate::ios_workspace::IosWorkspace::open(selection)
                    .map_err(|error| error.to_string())?;
                (WorkspaceSlot::ios(workspace), Some(bookmark))
            }
            SavedWorkspace::Local { .. } => {
                return Err("This favorite needs to be selected again so iOS can restore its folder permission".to_string());
            }
            SavedWorkspace::Smb(config) => {
                let password = crate::ios_bridge::load_smb_password(&smb_keychain_account(&config))
                    .ok_or_else(|| {
                        "The saved SMB password is unavailable in Keychain; reconnect to this share"
                            .to_string()
                    })?;
                let workspace = SmbWorkspace::connect(SmbConnectionConfig {
                    server: config.server,
                    share: config.share,
                    username: config.username,
                    password,
                    remote_path: config.remote_path,
                })
                .map_err(|error| error.to_string())?;
                (WorkspaceSlot::smb(workspace), None)
            }
        };
        let entries = workspace.entries().map_err(|error| error.to_string())?;
        self.install_scanned(workspace, bookmark, entries)
    }

    fn workspace_asset_data(&self, link: &str) -> Result<Option<String>, String> {
        let (workspace, current) = {
            let inner = self.locked()?;
            (
                inner
                    .workspace
                    .shared_clone()
                    .ok_or_else(|| "No workspace is open".to_string())?,
                Self::current_file(&inner)?,
            )
        };
        let Some(id) = workspace.resolve_asset_link(&current, link) else {
            return Ok(None);
        };
        let key = AssetCacheKey {
            workspace: workspace.identity(),
            id: id.clone(),
        };
        if let Some(cached) = self.cached_asset(&key) {
            return Ok(Some(cached));
        }
        let data = workspace
            .asset_bytes(&id)
            .map_err(|error| error.to_string())?;
        let mime = match std::path::Path::new(&id)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            Some("svg") => "image/svg+xml",
            _ => "application/octet-stream",
        };
        let data_url = format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(data)
        );
        self.cache_asset(key, data_url.clone());
        Ok(Some(data_url))
    }
}

/// Run a command's work on Tauri's blocking thread pool.
///
/// Synchronous Tauri commands execute on the main thread, so workspace I/O
/// there freezes the whole UI. `#[tauri::command(async)]` is not a substitute:
/// it runs on an async worker, where SMB operations (which block on their own
/// Tokio runtime) would panic.
async fn run_blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("Markerup command stopped unexpectedly: {error}"))?
}

async fn with_backend<T, F>(app: AppHandle, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&MarkerupBackend) -> Result<T, String> + Send + 'static,
{
    run_blocking(move || work(app.state::<MarkerupBackend>().inner())).await
}

#[tauri::command]
pub async fn workspace_snapshot(app: AppHandle) -> Result<WorkspaceSnapshot, String> {
    with_backend(app, |backend| backend.workspace_snapshot()).await
}

#[tauri::command]
pub async fn restored_workspace(app: AppHandle) -> Result<Option<WorkspaceSnapshot>, String> {
    with_backend(app, |backend| backend.restored_workspace()).await
}

#[cfg(not(target_os = "ios"))]
#[tauri::command]
pub async fn open_local_workspace(
    path: String,
    app: AppHandle,
) -> Result<WorkspaceSnapshot, String> {
    with_backend(app, move |backend| {
        backend.open_local_workspace(PathBuf::from(path))
    })
    .await
}

#[cfg(target_os = "ios")]
#[tauri::command]
pub fn open_local_workspace(path: String) -> Result<WorkspaceSnapshot, String> {
    let _ = path;
    Err("Use the iOS folder picker to select a local workspace".to_string())
}

#[cfg(target_os = "ios")]
#[tauri::command]
pub async fn choose_ios_workspace(app: AppHandle) -> Result<Option<WorkspaceSnapshot>, String> {
    with_backend(app, |backend| {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        crate::ios_bridge::choose_workspace(move |result| {
            let _ = sender.send(result);
        });
        let selection = receiver
            .recv()
            .map_err(|_| "iOS folder picker stopped unexpectedly".to_string())??;
        let Some(selection) = selection else {
            return Ok(None);
        };
        backend.install_ios_selection(selection).map(Some)
    })
    .await
}

#[tauri::command]
pub async fn connect_smb(
    request: SmbConnectRequest,
    app: AppHandle,
) -> Result<WorkspaceSnapshot, String> {
    with_backend(app, move |backend| backend.connect_smb(request)).await
}

#[tauri::command]
pub async fn open_note(id: String, app: AppHandle) -> Result<NotePayload, String> {
    with_backend(app, move |backend| backend.open_note(id)).await
}

#[tauri::command]
pub async fn save_note(
    contents: String,
    force: bool,
    app: AppHandle,
) -> Result<WorkspaceSnapshot, String> {
    with_backend(app, move |backend| backend.save_note(&contents, force)).await
}

#[tauri::command]
pub async fn reload_note(app: AppHandle) -> Result<NotePayload, String> {
    with_backend(app, |backend| backend.reload_note()).await
}

#[tauri::command]
pub async fn refresh_workspace(
    editor_has_unsaved_changes: bool,
    app: AppHandle,
) -> Result<WorkspaceSnapshot, String> {
    with_backend(app, move |backend| {
        backend.refresh_workspace(editor_has_unsaved_changes)
    })
    .await
}

#[tauri::command]
pub async fn search_workspace(
    query: String,
    app: AppHandle,
) -> Result<Option<Vec<EntryId>>, String> {
    with_backend(app, move |backend| backend.search_workspace(&query)).await
}

#[tauri::command]
pub async fn workspace_assets(app: AppHandle) -> Result<Vec<WorkspaceEntry>, String> {
    with_backend(app, |backend| backend.workspace_assets()).await
}

#[tauri::command]
pub async fn create_note(
    parent: String,
    name: String,
    app: AppHandle,
) -> Result<NotePayload, String> {
    with_backend(app, move |backend| backend.create_note(&parent, &name)).await
}

#[tauri::command]
pub async fn create_folder(
    parent: String,
    name: String,
    app: AppHandle,
) -> Result<WorkspaceSnapshot, String> {
    with_backend(app, move |backend| backend.create_folder(&parent, &name)).await
}

#[tauri::command]
pub async fn rename_entry(
    id: String,
    name: String,
    app: AppHandle,
) -> Result<WorkspaceSnapshot, String> {
    with_backend(app, move |backend| backend.rename_entry(&id, &name)).await
}

#[tauri::command]
pub async fn move_entry(
    id: String,
    destination_parent: String,
    app: AppHandle,
) -> Result<WorkspaceSnapshot, String> {
    with_backend(app, move |backend| {
        backend.move_entry(&id, &destination_parent)
    })
    .await
}

#[tauri::command]
pub async fn delete_entry(id: String, app: AppHandle) -> Result<WorkspaceSnapshot, String> {
    with_backend(app, move |backend| backend.delete_entry(&id)).await
}

#[tauri::command]
pub async fn navigate_markdown_link(link: String, app: AppHandle) -> Result<NotePayload, String> {
    with_backend(app, move |backend| backend.navigate_markdown_link(&link)).await
}

#[tauri::command]
pub async fn go_back(app: AppHandle) -> Result<Option<NotePayload>, String> {
    with_backend(app, |backend| backend.step_history(false)).await
}

#[tauri::command]
pub async fn go_forward(app: AppHandle) -> Result<Option<NotePayload>, String> {
    with_backend(app, |backend| backend.step_history(true)).await
}

#[tauri::command]
pub async fn set_workspace_favorite(
    favorited: bool,
    app: AppHandle,
) -> Result<WorkspaceSnapshot, String> {
    with_backend(app, move |backend| {
        backend.set_workspace_favorite(favorited)
    })
    .await
}

#[tauri::command]
pub async fn open_favorite_workspace(
    index: usize,
    app: AppHandle,
) -> Result<WorkspaceSnapshot, String> {
    with_backend(app, move |backend| backend.open_favorite_workspace(index)).await
}

#[cfg(target_os = "ios")]
fn smb_keychain_account(config: &SavedSmbConfig) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        config.server, config.share, config.username, config.remote_path
    )
}

#[tauri::command]
pub async fn preview_document(source: String) -> Result<PreviewDocument, String> {
    run_blocking(move || Ok(parse_preview_document(&source))).await
}

#[tauri::command]
pub async fn toggle_markdown_task(source: String, task_offset: usize) -> Result<String, String> {
    run_blocking(move || {
        toggle_task_at_offset(&source, task_offset)
            .ok_or_else(|| "Could not locate the Markdown task".to_string())
    })
    .await
}

#[tauri::command]
pub async fn render_mermaid(source: String) -> Result<String, String> {
    run_blocking(move || render_mermaid_svg(&source)).await
}

fn render_mermaid_svg(source: &str) -> Result<String, String> {
    let renderer = HeadlessRenderer::new().with_site_config(MermaidConfig::from_value(json!({
        "theme": "base",
        "themeVariables": {
            "background": "#1c1c1e", "mainBkg": "#252b35", "primaryColor": "#273449",
            "primaryTextColor": "#f2f2f7", "primaryBorderColor": "#72b7ff", "lineColor": "#93c5fd",
            "secondaryColor": "#334155", "tertiaryColor": "#1f2937", "nodeTextColor": "#f2f2f7",
            "textColor": "#f2f2f7", "edgeLabelBackground": "#1c1c1e", "clusterBkg": "#202938",
            "clusterBorder": "#64748b", "titleColor": "#f8fafc", "noteBkgColor": "#3a321f",
            "noteTextColor": "#fef3c7", "noteBorderColor": "#fbbf24"
        }
    })));
    renderer
        .render_svg_resvg_safe_sync_with_diagram_id(
            &normalize_mermaid_source(source),
            "markerup-preview",
        )
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "No Mermaid diagram found".to_string())
}

#[tauri::command]
pub async fn workspace_asset_data(link: String, app: AppHandle) -> Result<Option<String>, String> {
    with_backend(app, move |backend| backend.workspace_asset_data(&link)).await
}

#[tauri::command]
pub fn privacy_policy_url() -> &'static str {
    PRIVACY_POLICY_URL
}

#[cfg(target_os = "ios")]
#[tauri::command]
pub fn finish_ios_background_save() {
    crate::ios_bridge::finish_background_task();
}

fn normalize_mermaid_source(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            let mut output = String::new();
            let mut chars = line.chars().peekable();
            while let Some(&character) = chars.peek() {
                if character != ' ' && character != '\t' {
                    break;
                }
                output.push_str(if chars.next() == Some('\t') {
                    "  "
                } else {
                    " "
                });
            }
            let rest: String = chars.collect();
            let mut index = 0;
            while let Some(found) = rest[index..].find('#') {
                let position = index + found;
                output.push_str(&rest[index..position]);
                let hex_count = rest[position + 1..]
                    .chars()
                    .take_while(|character| character.is_ascii_hexdigit())
                    .count();
                if matches!(hex_count, 3 | 4 | 6 | 8) {
                    output.push('#');
                } else {
                    output.push_str("Number");
                }
                index = position + 1;
            }
            output.push_str(&rest[index..]);
            output
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        AssetCache, AssetCacheKey, BackendInner, MAX_CACHED_ASSETS, MarkerupBackend,
        PendingRestore, preview_document,
    };
    use crate::persistence::{SavedFavorite, SavedWorkspace};
    use crate::workspace::{LocalWorkspace, Workspace, WorkspaceSlot};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn temp_workspace(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("markerup-backend-{name}-{unique}"));
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("Groceries.md"), "# Groceries\nmilk").unwrap();
        fs::write(root.join("nested/Ideas.md"), "# Ideas\nbuy MILK in bulk").unwrap();
        fs::write(root.join("nested/Other.md"), "# Other").unwrap();
        root
    }

    /// Install a workspace without `install_workspace`, which would persist
    /// (and could clear) the real user session file.
    fn backend_with_workspace(root: &PathBuf) -> MarkerupBackend {
        let backend = MarkerupBackend::default();
        let workspace = LocalWorkspace::open(root).unwrap();
        {
            let mut inner = backend.locked().unwrap();
            inner.entries = workspace.entries().unwrap();
            inner.workspace = WorkspaceSlot::local(workspace);
        }
        backend
    }

    #[test]
    fn preview_command_sends_reference_link_definitions() {
        let source = "[Home][home]\n\n[home]: ../index.md";
        let document =
            tauri::async_runtime::block_on(preview_document(source.to_string())).unwrap();
        let payload = serde_json::to_value(&document).unwrap();
        assert_eq!(
            payload["linkDefinitions"],
            serde_json::json!(["[home]: ../index.md"])
        );
        assert!(payload["blocks"].is_array());
    }

    #[test]
    fn search_matches_names_and_contents_within_the_scanned_tree() {
        let root = temp_workspace("search");
        let backend = backend_with_workspace(&root);
        let mut results = backend.search_workspace(" milk ").unwrap().unwrap();
        results.sort();
        assert_eq!(results, vec!["Groceries.md", "nested/Ideas.md"]);
        assert_eq!(
            backend.search_workspace("other").unwrap().unwrap(),
            vec!["nested/Other.md"]
        );
        assert_eq!(
            backend.search_workspace("  ").unwrap().unwrap(),
            Vec::<String>::new()
        );
        // A note added outside Markerup appears once the tree is refreshed,
        // matching what the sidebar can show.
        fs::write(root.join("Later.md"), "milk").unwrap();
        assert!(
            !backend
                .search_workspace("milk")
                .unwrap()
                .unwrap()
                .contains(&"Later.md".to_string())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_never_replaces_a_workspace_chosen_while_it_ran() {
        let root = temp_workspace("restore");
        let backend = Arc::new(MarkerupBackend::default());
        backend.locked().unwrap().restoring = true;
        let waiter = {
            let backend = Arc::clone(&backend);
            std::thread::spawn(move || backend.restored_workspace())
        };
        // The user picks another workspace before the favorite reopens.
        backend.locked().unwrap().workspace_revision += 1;
        backend.finish_restore(PendingRestore {
            favorite: SavedFavorite {
                workspace: SavedWorkspace::Local {
                    path: root.clone(),
                    bookmark: None,
                },
            },
            current_file: Some("Groceries.md".to_string()),
            revision: 0,
        });
        assert!(waiter.join().unwrap().unwrap().is_none());
        let inner = backend.locked().unwrap();
        assert!(!inner.restoring);
        assert!(!inner.workspace.is_open());
        assert!(inner.current_file.is_none());
        drop(inner);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn waiting_for_restore_returns_immediately_when_none_is_pending() {
        let backend = MarkerupBackend::default();
        assert!(backend.restored_workspace().unwrap().is_none());
    }

    #[test]
    fn external_refresh_never_advances_the_save_baseline_without_loading_the_editor() {
        let mut inner = BackendInner {
            disk_text: "# Original".into(),
            ..Default::default()
        };
        MarkerupBackend::reconcile_disk_change(&mut inner, "# External update", false);
        assert_eq!(inner.disk_text, "# Original");
        assert!(inner.external_conflict);
        MarkerupBackend::reconcile_disk_change(&mut inner, "# Original", false);
        assert!(!inner.external_conflict);
    }

    #[test]
    fn recovers_workspace_state_after_a_panicking_command() {
        let backend = MarkerupBackend::default();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _inner = backend.inner.lock().expect("fresh state lock");
            panic!("simulated command failure");
        }));

        assert!(backend.inner.lock().is_err());
        assert!(backend.locked().is_ok());
    }

    #[test]
    fn asset_cache_reuses_entries_and_evicts_oldest_entries() {
        let mut cache = AssetCache::default();
        let first = AssetCacheKey {
            workspace: "workspace".to_string(),
            id: "first.png".to_string(),
        };
        cache.insert(first.clone(), "data:image/png;base64,first".to_string());
        assert_eq!(
            cache.get(&first).as_deref(),
            Some("data:image/png;base64,first")
        );

        for index in 0..MAX_CACHED_ASSETS {
            cache.insert(
                AssetCacheKey {
                    workspace: "workspace".to_string(),
                    id: format!("asset-{index}.png"),
                },
                format!("data:image/png;base64,{index}"),
            );
        }

        assert!(cache.entries.len() <= MAX_CACHED_ASSETS);
        assert!(cache.get(&first).is_none());
        cache.clear();
        assert!(cache.entries.is_empty());
        assert_eq!(cache.used_bytes, 0);
    }
}
