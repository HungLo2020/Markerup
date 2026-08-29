use crate::markdown::{
    PreviewBlock, preview_document as parse_preview_document, toggle_task_at_offset,
};
use crate::navigation::NavigationState;
use crate::persistence::{SavedSmbConfig, clear_session, load_session, save_session};
use crate::smb_workspace::{SmbConnectionConfig, SmbWorkspace};
#[cfg(not(target_os = "ios"))]
use crate::workspace::LocalWorkspace;
use crate::workspace::{
    EntryId, LinkTarget, Workspace, WorkspaceEntry, WorkspaceRef, WorkspaceSlot,
};
use base64::Engine;
use merman::MermaidConfig;
use merman::render::HeadlessRenderer;
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(not(target_os = "ios"))]
use std::path::PathBuf;
use std::sync::Mutex;

const PRIVACY_POLICY_URL: &str = "https://hunglo2020.github.io/Markerup/privacy-policy/";

#[derive(Default)]
struct BackendInner {
    workspace: WorkspaceSlot,
    entries: Vec<WorkspaceEntry>,
    pinned: bool,
    bookmark: Option<Vec<u8>>,
    current_file: Option<EntryId>,
    disk_text: String,
    external_conflict: bool,
    navigation: NavigationState,
}

/// The sole owner of canonical workspace state. The web UI keeps only the
/// active editor buffer; every filesystem/SMB mutation is performed here.
#[derive(Default)]
pub struct MarkerupBackend {
    inner: Mutex<BackendInner>,
}

struct SaveRequest {
    workspace: WorkspaceRef,
    file: EntryId,
    baseline: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub workspace_open: bool,
    pub workspace_path: String,
    pub workspace_is_smb: bool,
    pub workspace_pinned: bool,
    pub entries: Vec<WorkspaceEntry>,
    pub current_file: Option<EntryId>,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub external_conflict: bool,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPayload {
    pub blocks: Vec<PreviewBlock>,
}

impl MarkerupBackend {
    fn locked(&self) -> Result<std::sync::MutexGuard<'_, BackendInner>, String> {
        self.inner
            .lock()
            .map_err(|_| "Markerup workspace state is unavailable".to_string())
    }

    fn snapshot(inner: &BackendInner) -> WorkspaceSnapshot {
        let workspace_open = inner.workspace.is_open();
        WorkspaceSnapshot {
            workspace_open,
            workspace_path: inner.workspace.root_display(),
            workspace_is_smb: matches!(inner.workspace, WorkspaceSlot::Smb(_)),
            workspace_pinned: inner.pinned,
            entries: inner.entries.clone(),
            current_file: inner.current_file.clone(),
            can_go_back: inner.navigation.can_go_back(),
            can_go_forward: inner.navigation.can_go_forward(),
            external_conflict: inner.external_conflict,
        }
    }

    fn refresh_entries(inner: &mut BackendInner) -> Result<(), String> {
        inner.entries = if inner.workspace.is_open() {
            inner
                .workspace
                .entries()
                .map_err(|error| error.to_string())?
        } else {
            Vec::new()
        };
        Ok(())
    }

    fn persist(inner: &BackendInner) {
        if !inner.pinned {
            let _ = clear_session();
            return;
        }
        let smb = inner.workspace.smb_config().map(|config| SavedSmbConfig {
            server: config.server,
            share: config.share,
            username: config.username,
            remote_path: config.remote_path,
        });
        let _ = save_session(
            inner.workspace.root_path(),
            inner.current_file.as_deref(),
            inner.bookmark.as_deref(),
            smb.as_ref(),
        );
    }

    fn install_workspace(
        inner: &mut BackendInner,
        workspace: WorkspaceSlot,
        pinned: bool,
        bookmark: Option<Vec<u8>>,
    ) {
        inner.workspace = workspace;
        inner.entries.clear();
        inner.pinned = pinned;
        inner.bookmark = bookmark;
        inner.current_file = None;
        inner.disk_text.clear();
        inner.external_conflict = false;
        inner.navigation = NavigationState::default();
        Self::persist(inner);
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

    pub fn restore(&self) {
        let Some(session) = load_session() else {
            return;
        };
        let Ok(mut inner) = self.locked() else {
            return;
        };
        #[cfg(not(target_os = "ios"))]
        {
            if let Ok(workspace) = LocalWorkspace::open(&session.pinned_workspace) {
                Self::install_workspace(&mut inner, WorkspaceSlot::local(workspace), true, None);
                let _ = Self::refresh_entries(&mut inner);
                if let Some(id) = session.current_file {
                    let _ = Self::open_note_locked(&mut inner, id, false);
                }
            }
        }
        #[cfg(target_os = "ios")]
        {
            if let Some(smb) = session.smb {
                let account = format!(
                    "{}\n{}\n{}\n{}",
                    smb.server, smb.share, smb.username, smb.remote_path
                );
                if let Some(password) = crate::ios_bridge::load_smb_password(&account) {
                    let config = SmbConnectionConfig {
                        server: smb.server,
                        share: smb.share,
                        username: smb.username,
                        password,
                        remote_path: smb.remote_path,
                    };
                    if let Ok(workspace) = SmbWorkspace::connect(config) {
                        Self::install_workspace(
                            &mut inner,
                            WorkspaceSlot::smb(workspace),
                            true,
                            None,
                        );
                        let _ = Self::refresh_entries(&mut inner);
                    }
                }
            } else if let Some(bookmark) = session.bookmark
                && let Ok(selection) = crate::ios_bridge::resolve_bookmark(&bookmark)
                && let Ok(workspace) = crate::ios_workspace::IosWorkspace::open(selection)
            {
                Self::install_workspace(
                    &mut inner,
                    WorkspaceSlot::ios(workspace),
                    true,
                    Some(bookmark),
                );
                let _ = Self::refresh_entries(&mut inner);
            }
            if let Some(id) = session.current_file {
                let _ = Self::open_note_locked(&mut inner, id, false);
            }
        }
    }
}

#[tauri::command]
pub fn workspace_snapshot(
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<WorkspaceSnapshot, String> {
    let inner = state.locked()?;
    Ok(MarkerupBackend::snapshot(&inner))
}

#[tauri::command]
pub fn open_local_workspace(
    path: String,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<WorkspaceSnapshot, String> {
    #[cfg(target_os = "ios")]
    let _ = (path, state);
    #[cfg(not(target_os = "ios"))]
    {
        let workspace =
            LocalWorkspace::open(PathBuf::from(path)).map_err(|error| error.to_string())?;
        let entries = workspace.entries().map_err(|error| error.to_string())?;
        let mut inner = state.locked()?;
        MarkerupBackend::install_workspace(
            &mut inner,
            WorkspaceSlot::local(workspace),
            false,
            None,
        );
        inner.entries = entries;
        Ok(MarkerupBackend::snapshot(&inner))
    }
    #[cfg(target_os = "ios")]
    Err("Use the iOS folder picker to select a local workspace".to_string())
}

#[cfg(target_os = "ios")]
#[tauri::command]
pub async fn choose_ios_workspace(
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<Option<WorkspaceSnapshot>, String> {
    let selection = tauri::async_runtime::spawn_blocking(|| {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        crate::ios_bridge::choose_workspace(move |result| {
            let _ = sender.send(result);
        });
        receiver
            .recv()
            .map_err(|_| "iOS folder picker stopped unexpectedly".to_string())?
    })
    .await
    .map_err(|error| error.to_string())??;
    let Some(selection) = selection else {
        return Ok(None);
    };
    let bookmark = selection.bookmark.clone();
    let workspace =
        crate::ios_workspace::IosWorkspace::open(selection).map_err(|error| error.to_string())?;
    let entries = workspace.entries().map_err(|error| error.to_string())?;
    let mut inner = state.locked()?;
    MarkerupBackend::install_workspace(
        &mut inner,
        WorkspaceSlot::ios(workspace),
        false,
        Some(bookmark),
    );
    inner.entries = entries;
    Ok(Some(MarkerupBackend::snapshot(&inner)))
}

#[tauri::command]
pub fn connect_smb(
    request: SmbConnectRequest,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<WorkspaceSnapshot, String> {
    let config = SmbConnectionConfig {
        server: request.server,
        share: request.share,
        username: request.username,
        password: request.password,
        remote_path: request.remote_path,
    };
    let workspace = SmbWorkspace::connect(config).map_err(|error| error.to_string())?;
    let entries = workspace.entries().map_err(|error| error.to_string())?;
    let mut inner = state.locked()?;
    MarkerupBackend::install_workspace(&mut inner, WorkspaceSlot::smb(workspace), false, None);
    inner.entries = entries;
    Ok(MarkerupBackend::snapshot(&inner))
}

#[tauri::command]
pub fn open_note(
    id: String,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<NotePayload, String> {
    let mut inner = state.locked()?;
    MarkerupBackend::open_note_locked(&mut inner, id, true)
}

#[tauri::command]
pub fn save_note(
    contents: String,
    force: bool,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<WorkspaceSnapshot, String> {
    let request = state.begin_save(force)?;
    let current_disk = request
        .workspace
        .read(&request.file)
        .map_err(|error| error.to_string())?;
    if !force && current_disk != request.baseline {
        state.mark_conflict_if_current(&request);
        return Err("External change conflict".to_string());
    }
    if let Err(error) = request.workspace.write(&request.file, &contents) {
        if error.kind() == std::io::ErrorKind::Interrupted {
            return match request.workspace.read(&request.file) {
                Ok(remote) if remote == contents => state.complete_save(&request, &contents),
                Ok(_) => {
                    state.mark_conflict_if_current(&request);
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
        state.mark_conflict_if_current(&request);
        return Err("External change conflict — the note changed while Markerup was saving. Reload before retrying.".to_string());
    }
    state.complete_save(&request, &contents)
}

#[tauri::command]
pub fn reload_note(state: tauri::State<'_, MarkerupBackend>) -> Result<NotePayload, String> {
    let mut inner = state.locked()?;
    let id = MarkerupBackend::current_file(&inner)?;
    MarkerupBackend::open_note_locked(&mut inner, id, false)
}

#[tauri::command]
pub fn refresh_workspace(
    editor_has_unsaved_changes: bool,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<WorkspaceSnapshot, String> {
    let mut inner = state.locked()?;
    if let Some(file) = inner.current_file.clone() {
        let disk = inner
            .workspace
            .read(&file)
            .map_err(|error| error.to_string())?;
        if disk != inner.disk_text && editor_has_unsaved_changes {
            inner.external_conflict = true;
        } else if disk != inner.disk_text {
            inner.disk_text = disk;
            inner.external_conflict = false;
        }
    }
    MarkerupBackend::refresh_entries(&mut inner)?;
    Ok(MarkerupBackend::snapshot(&inner))
}

#[tauri::command]
pub fn search_workspace(
    query: String,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<Vec<EntryId>, String> {
    state
        .locked()?
        .workspace
        .search_markdown(&query)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn create_note(
    parent: String,
    name: String,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<NotePayload, String> {
    let mut inner = state.locked()?;
    let id = inner
        .workspace
        .create_note(&parent, &name)
        .map_err(|error| error.to_string())?;
    MarkerupBackend::refresh_entries(&mut inner)?;
    MarkerupBackend::open_note_locked(&mut inner, id, true)
}

#[tauri::command]
pub fn create_folder(
    parent: String,
    name: String,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<WorkspaceSnapshot, String> {
    let mut inner = state.locked()?;
    inner
        .workspace
        .create_directory(&parent, &name)
        .map_err(|error| error.to_string())?;
    MarkerupBackend::refresh_entries(&mut inner)?;
    Ok(MarkerupBackend::snapshot(&inner))
}

#[tauri::command]
pub fn rename_entry(
    id: String,
    name: String,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<WorkspaceSnapshot, String> {
    let mut inner = state.locked()?;
    let new_id = inner
        .workspace
        .rename(&id, &name)
        .map_err(|error| error.to_string())?;
    inner.navigation.rebase(&id, &new_id);
    if inner.current_file.as_deref() == Some(id.as_str()) {
        inner.current_file = Some(new_id);
    }
    MarkerupBackend::persist(&inner);
    MarkerupBackend::refresh_entries(&mut inner)?;
    Ok(MarkerupBackend::snapshot(&inner))
}

#[tauri::command]
pub fn delete_entry(
    id: String,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<WorkspaceSnapshot, String> {
    let mut inner = state.locked()?;
    inner
        .workspace
        .delete(&id)
        .map_err(|error| error.to_string())?;
    inner.navigation.remove(&id);
    if inner.current_file.as_deref() == Some(id.as_str())
        || inner
            .current_file
            .as_deref()
            .is_some_and(|file| file.starts_with(&(id.clone() + "/")))
    {
        inner.current_file = None;
        inner.disk_text.clear();
    }
    MarkerupBackend::persist(&inner);
    MarkerupBackend::refresh_entries(&mut inner)?;
    Ok(MarkerupBackend::snapshot(&inner))
}

#[tauri::command]
pub fn navigate_markdown_link(
    link: String,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<NotePayload, String> {
    let mut inner = state.locked()?;
    let current = MarkerupBackend::current_file(&inner)?;
    let LinkTarget { id, .. } = inner
        .workspace
        .resolve_markdown_link(&current, &link)
        .ok_or_else(|| "Link does not point to a Markdown note in this workspace".to_string())?;
    MarkerupBackend::open_note_locked(&mut inner, id, true)
}

#[tauri::command]
pub fn go_back(state: tauri::State<'_, MarkerupBackend>) -> Result<Option<NotePayload>, String> {
    let mut inner = state.locked()?;
    let current = inner.current_file.clone();
    let Some(target) = inner.navigation.go_back(current.as_deref()) else {
        return Ok(None);
    };
    MarkerupBackend::open_note_locked(&mut inner, target, false).map(Some)
}

#[tauri::command]
pub fn go_forward(state: tauri::State<'_, MarkerupBackend>) -> Result<Option<NotePayload>, String> {
    let mut inner = state.locked()?;
    let current = inner.current_file.clone();
    let Some(target) = inner.navigation.go_forward(current.as_deref()) else {
        return Ok(None);
    };
    MarkerupBackend::open_note_locked(&mut inner, target, false).map(Some)
}

#[tauri::command]
pub fn set_workspace_pinned(
    pinned: bool,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<WorkspaceSnapshot, String> {
    let mut inner = state.locked()?;
    if matches!(inner.workspace, WorkspaceSlot::Smb(_)) {
        #[cfg(target_os = "ios")]
        if let Some(config) = inner.workspace.smb_config() {
            if pinned {
                crate::ios_bridge::save_smb_password(&config.keychain_account(), &config.password)?;
            } else {
                crate::ios_bridge::delete_smb_password(&config.keychain_account());
            }
        }
        #[cfg(not(target_os = "ios"))]
        if pinned {
            return Err("SMB credentials remain session-only on Linux".to_string());
        }
    }
    inner.pinned = pinned;
    MarkerupBackend::persist(&inner);
    Ok(MarkerupBackend::snapshot(&inner))
}

#[tauri::command]
pub fn preview_document(source: String) -> PreviewPayload {
    PreviewPayload {
        blocks: parse_preview_document(&source).blocks,
    }
}

#[tauri::command]
pub fn toggle_markdown_task(source: String, task_offset: usize) -> Result<String, String> {
    toggle_task_at_offset(&source, task_offset)
        .ok_or_else(|| "Could not locate the Markdown task".to_string())
}

#[tauri::command]
pub fn render_mermaid(source: String) -> Result<String, String> {
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
            &normalize_mermaid_source(&source),
            "markerup-preview",
        )
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "No Mermaid diagram found".to_string())
}

#[tauri::command]
pub fn workspace_asset_data(
    link: String,
    state: tauri::State<'_, MarkerupBackend>,
) -> Result<Option<String>, String> {
    let inner = state.locked()?;
    let current = MarkerupBackend::current_file(&inner)?;
    let Some(id) = inner.workspace.resolve_asset_link(&current, &link) else {
        return Ok(None);
    };
    let Some(path) = inner
        .workspace
        .asset_path(&id)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let data = std::fs::read(&path).map_err(|error| error.to_string())?;
    let mime = match path
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
    Ok(Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(data)
    )))
}

#[tauri::command]
pub fn privacy_policy_url() -> &'static str {
    PRIVACY_POLICY_URL
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
