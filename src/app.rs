use crate::markdown::{image_references, preview_markdown};
use crate::persistence::{clear_session, save_session};
use crate::workspace::{EntryId, EntryKind, Workspace, WorkspaceEntry, WorkspaceSlot};
use crate::MainWindow;
use slint::{Image, ModelRc, SharedString, StyledText, VecModel};
use std::collections::HashSet;
use std::time::{Duration, Instant};

pub struct AppState {
    pub workspace: WorkspaceSlot,
    pub pinned: bool,
    pub entries: Vec<WorkspaceEntry>,
    pub tree_ids: Vec<EntryId>,
    pub expanded: HashSet<EntryId>,
    expansion_initialized: bool,
    pub selected: Option<EntryId>,
    pub current_file: Option<EntryId>,
    pub disk_text: String,
    pub dirty: bool,
    pub external_conflict: bool,
    pub back: Vec<EntryId>,
    pub forward: Vec<EntryId>,
    pub search_results: Vec<EntryId>,
    pub find_query: String,
    pub find_matches: Vec<(usize, usize)>,
    pub find_index: usize,
    pub delete_armed: Option<(EntryId, Instant)>,
}

impl AppState {
    pub fn new(workspace: WorkspaceSlot, pinned: bool) -> Self {
        Self {
            workspace,
            pinned,
            entries: Vec::new(), tree_ids: Vec::new(), expanded: HashSet::new(),
            expansion_initialized: false, selected: None, current_file: None,
            disk_text: String::new(), dirty: false, external_conflict: false,
            back: Vec::new(), forward: Vec::new(), search_results: Vec::new(),
            find_query: String::new(), find_matches: Vec::new(), find_index: 0,
            delete_armed: None,
        }
    }

    pub fn replace_workspace(&mut self, workspace: WorkspaceSlot, pinned: bool) {
        *self = Self::new(workspace, pinned);
    }

    pub fn refresh_entries(&mut self) -> std::io::Result<()> {
        self.entries = self.workspace.entries()?;
        if !self.expansion_initialized {
            self.expanded.extend(self.entries.iter()
                .filter(|entry| entry.kind == EntryKind::Directory)
                .map(|entry| entry.id.clone()));
            self.expansion_initialized = true;
        }
        Ok(())
    }

    pub fn entry(&self, id: &str) -> Option<&WorkspaceEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn selected_parent(&self) -> EntryId {
        if let Some(selected) = self.selected.as_deref() {
            if self.entry(selected).is_some_and(|e| e.kind == EntryKind::Directory) {
                return selected.to_string();
            }
            return parent_id(selected);
        }
        self.current_file.as_deref().map(parent_id).unwrap_or_default()
    }

    pub fn current_is_under(&self, id: &str) -> bool {
        self.current_file.as_deref()
            .is_some_and(|current| current == id || current.starts_with(&format!("{id}/")))
    }

    pub fn delete_is_armed(&self, id: &str) -> bool {
        self.delete_armed.as_ref().is_some_and(|(armed, when)|
            armed == id && when.elapsed() < Duration::from_secs(5))
    }
}

pub fn string_model(values: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(values.into_iter().map(SharedString::from).collect::<Vec<_>>()))
}

fn image_model(values: Vec<Image>) -> ModelRc<Image> { ModelRc::new(VecModel::from(values)) }

pub fn parent_id(id: &str) -> EntryId {
    id.rsplit_once('/').map(|(parent, _)| parent.to_string()).unwrap_or_default()
}

fn ancestor_dirs(id: &str) -> Vec<EntryId> {
    let mut ancestors = Vec::new();
    let mut current = parent_id(id);
    while !current.is_empty() {
        ancestors.push(current.clone());
        current = parent_id(&current);
    }
    ancestors
}

pub fn rebase_id(id: &str, old: &str, new: &str) -> EntryId {
    if id == old { return new.to_string(); }
    id.strip_prefix(&format!("{old}/"))
        .map(|suffix| format!("{new}/{suffix}"))
        .unwrap_or_else(|| id.to_string())
}

pub fn set_status(ui: &MainWindow, text: impl Into<SharedString>) { ui.set_status(text.into()); }

pub fn sync_flags(ui: &MainWindow, state: &AppState) {
    ui.set_workspace_open(state.workspace.is_open());
    ui.set_workspace_pinned(state.pinned);
    ui.set_workspace_path(state.workspace.root_display().into());
    ui.set_dirty(state.dirty);
    ui.set_external_conflict(state.external_conflict);
    ui.set_can_go_back(!state.back.is_empty());
    ui.set_can_go_forward(!state.forward.is_empty());
}

pub fn render_tree(ui: &MainWindow, state: &mut AppState) {
    let mut labels = Vec::new();
    let mut ids = Vec::new();
    for entry in &state.entries {
        if ancestor_dirs(&entry.id).iter().any(|a| !state.expanded.contains(a)) { continue; }
        let indent = "    ".repeat(entry.depth);
        let marker = if state.selected.as_deref() == Some(entry.id.as_str()) { "• " } else { "  " };
        let kind = match entry.kind {
            EntryKind::Directory if state.expanded.contains(&entry.id) => "▾ ",
            EntryKind::Directory => "▸ ",
            EntryKind::File => "  ",
        };
        labels.push(format!("{marker}{indent}{kind}{}", entry.name));
        ids.push(entry.id.clone());
    }
    state.tree_ids = ids;
    ui.set_tree_labels(string_model(labels));
    ui.set_selected_path(state.selected.clone().unwrap_or_default().into());
}

pub fn set_preview(ui: &MainWindow, state: &AppState, source: &str) {
    let compatible = preview_markdown(source);
    let styled = StyledText::from_markdown(&compatible)
        .unwrap_or_else(|_| StyledText::from_plain_text(source));
    ui.set_preview(styled);

    let mut images = Vec::new();
    let mut labels = Vec::new();
    if let Some(current) = state.current_file.as_deref() {
        for reference in image_references(source) {
            let Some(asset_id) = state.workspace.resolve_asset_link(current, &reference.destination) else { continue };
            let Ok(path) = state.workspace.absolute_asset_path(&asset_id) else { continue };
            if let Ok(image) = Image::load_from_path(&path) {
                labels.push(if reference.alt.is_empty() { asset_id } else { reference.alt });
                images.push(image);
            }
        }
    }
    ui.set_preview_images(image_model(images));
    ui.set_preview_image_labels(string_model(labels));
}

pub fn save_session_for(state: &AppState) {
    if state.pinned {
        if let Some(root) = state.workspace.root_path() {
            let _ = save_session(root, state.current_file.as_deref());
            return;
        }
    }
    let _ = clear_session();
}

pub fn clear_current(ui: &MainWindow, state: &mut AppState) {
    state.current_file = None;
    state.disk_text.clear();
    state.dirty = false;
    state.external_conflict = false;
    ui.set_current_path("No note selected".into());
    ui.set_editor_text("".into());
    ui.set_preview(StyledText::from_plain_text(""));
    ui.set_preview_images(image_model(Vec::new()));
    ui.set_preview_image_labels(string_model(Vec::new()));
    sync_flags(ui, state);
    save_session_for(state);
}

pub fn reset_workspace_ui(ui: &MainWindow, state: &mut AppState) {
    ui.set_selected_path("".into());
    ui.set_tree_labels(string_model(Vec::new()));
    ui.set_search_results(string_model(Vec::new()));
    ui.set_action_name("".into());
    ui.set_search_query("".into());
    ui.set_find_query("".into());
    ui.set_find_status("".into());
    clear_current(ui, state);
    render_tree(ui, state);
    sync_flags(ui, state);
}

pub fn open_file(ui: &MainWindow, state: &mut AppState, id: EntryId, history: bool) -> bool {
    if state.dirty && state.current_file.as_deref() != Some(id.as_str()) {
        set_status(ui, "Unsaved changes: save or reload before leaving this note");
        return false;
    }
    let contents = match state.workspace.read(&id) {
        Ok(contents) => contents,
        Err(error) => { set_status(ui, format!("Open failed: {error}")); return false; }
    };
    if history {
        if let Some(current) = state.current_file.clone() {
            if current != id { state.back.push(current); state.forward.clear(); }
        }
    }
    state.current_file = Some(id.clone());
    state.selected = Some(id.clone());
    state.disk_text = contents.clone();
    state.dirty = false;
    state.external_conflict = false;
    state.find_query.clear(); state.find_matches.clear(); state.find_index = 0;
    ui.set_current_path(id.into());
    ui.set_editor_text(contents.clone().into());
    ui.set_find_status("".into());
    set_preview(ui, state, &contents);
    render_tree(ui, state);
    sync_flags(ui, state);
    set_status(ui, "Ready");
    save_session_for(state);
    true
}

pub fn save_current(ui: &MainWindow, state: &mut AppState, contents: &str, force: bool) {
    let Some(current) = state.current_file.clone() else { set_status(ui, "No note selected"); return; };
    if state.external_conflict && !force {
        set_status(ui, "File changed externally. Reload external changes or choose Overwrite.");
        return;
    }
    match state.workspace.write(&current, contents) {
        Ok(()) => {
            state.disk_text = contents.to_string(); state.dirty = false; state.external_conflict = false;
            sync_flags(ui, state); set_status(ui, format!("Saved {current}"));
        }
        Err(error) => set_status(ui, format!("Save failed: {error}")),
    }
}

pub fn reload_current(ui: &MainWindow, state: &mut AppState) {
    let Some(current) = state.current_file.clone() else { return; };
    match state.workspace.read(&current) {
        Ok(contents) => {
            state.disk_text = contents.clone(); state.dirty = false; state.external_conflict = false;
            ui.set_editor_text(contents.clone().into()); set_preview(ui, state, &contents);
            sync_flags(ui, state); set_status(ui, "Reloaded from disk");
        }
        Err(error) => set_status(ui, format!("Reload failed: {error}")),
    }
}

pub fn refresh_workspace(ui: &MainWindow, state: &mut AppState) {
    if !state.workspace.is_open() { return; }
    if let Err(error) = state.refresh_entries() { set_status(ui, format!("Refresh failed: {error}")); return; }
    render_tree(ui, state);
    let Some(current) = state.current_file.clone() else { return; };
    match state.workspace.read(&current) {
        Ok(disk) if disk != state.disk_text => {
            if state.dirty {
                state.external_conflict = true; sync_flags(ui, state);
                set_status(ui, "CONFLICT: this note changed externally while you have unsaved edits");
            } else {
                state.disk_text = disk.clone(); ui.set_editor_text(disk.clone().into());
                set_preview(ui, state, &disk); set_status(ui, "Reloaded external change");
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if state.dirty {
                state.external_conflict = true; sync_flags(ui, state);
                set_status(ui, "CONFLICT: current note was deleted or moved externally");
            } else { clear_current(ui, state); set_status(ui, "Current note was removed externally"); }
        }
        Err(error) => set_status(ui, format!("External refresh failed: {error}")),
    }
}

pub fn mutate_refresh(ui: &MainWindow, state: &mut AppState) {
    match state.refresh_entries() {
        Ok(()) => render_tree(ui, state),
        Err(error) => set_status(ui, format!("Refresh failed: {error}")),
    }
}
