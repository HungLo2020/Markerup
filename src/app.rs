use crate::MainWindow;
use crate::markdown::PreviewBlockKind;
use crate::persistence::{SavedSmbConfig, clear_session, save_session};
use crate::workers::{
    LATEST_PREVIEW_GENERATION, LATEST_SEARCH_GENERATION, PreviewResult, ScanResult, SearchResult,
    hash_text,
};
use crate::workspace::{EntryId, EntryKind, Workspace, WorkspaceEntry, WorkspaceSlot};
use slint::{Image, ModelRc, SharedString, StyledText, VecModel};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

pub const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(200);
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);
pub const SCAN_DEBOUNCE: Duration = Duration::from_millis(100);

fn set_editor_text(ui: &MainWindow, text: String) {
    // Slint's mobile TextEdit does not expose the gesture-panning switch of
    // its internal scroll view. The mobile shell therefore scrolls an
    // intrinsic-height editor surface; estimate wrapped Markdown lines so the
    // surface remains large enough for touch panning without growing to an
    // unbounded fixed size.
    let visual_lines: usize = text
        .lines()
        .map(|line| line.chars().count().max(1).div_ceil(48))
        .sum::<usize>()
        .max(20);
    ui.set_editor_content_height((visual_lines.saturating_mul(22).max(480)) as f32);
    ui.set_editor_text(text.into());
}

#[derive(Debug)]
pub struct PendingPreview {
    pub generation: u64,
    pub due: Instant,
    pub source: String,
}

#[derive(Debug)]
pub struct PendingSearch {
    pub generation: u64,
    pub due: Instant,
    pub query: String,
}

#[derive(Debug)]
pub struct PendingScan {
    pub generation: u64,
    pub due: Instant,
    pub full_tree: bool,
}

#[derive(Clone)]
struct CachedImage {
    modified: Option<SystemTime>,
    len: u64,
    image: Image,
}

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
    pub saved_hash: u64,
    pub dirty: bool,
    pub external_conflict: bool,
    pub back: Vec<EntryId>,
    pub forward: Vec<EntryId>,
    pub search_results: Vec<EntryId>,
    pub find_query: String,
    pub find_matches: Vec<(usize, usize)>,
    pub find_index: usize,
    pub delete_armed: Option<(EntryId, Instant)>,
    pub preview_generation: u64,
    pub search_generation: u64,
    pub scan_generation: u64,
    pub pending_preview: Option<PendingPreview>,
    pub pending_search: Option<PendingSearch>,
    pub pending_scan: Option<PendingScan>,
    pub tree_model: Rc<VecModel<SharedString>>,
    image_cache: HashMap<EntryId, CachedImage>,
}

impl AppState {
    pub fn new(workspace: WorkspaceSlot, pinned: bool) -> Self {
        Self {
            workspace,
            pinned,
            entries: Vec::new(),
            tree_ids: Vec::new(),
            expanded: HashSet::new(),
            expansion_initialized: false,
            selected: None,
            current_file: None,
            disk_text: String::new(),
            saved_hash: hash_text(""),
            dirty: false,
            external_conflict: false,
            back: Vec::new(),
            forward: Vec::new(),
            search_results: Vec::new(),
            find_query: String::new(),
            find_matches: Vec::new(),
            find_index: 0,
            delete_armed: None,
            preview_generation: 0,
            search_generation: 0,
            scan_generation: 0,
            pending_preview: None,
            pending_search: None,
            pending_scan: None,
            tree_model: Rc::new(VecModel::from(Vec::<SharedString>::new())),
            image_cache: HashMap::new(),
        }
    }

    pub fn replace_workspace(&mut self, workspace: WorkspaceSlot, pinned: bool) {
        *self = Self::new(workspace, pinned);
    }

    pub fn apply_entries(&mut self, entries: Vec<WorkspaceEntry>) {
        self.entries = entries;
        if !self.expansion_initialized {
            self.expanded.extend(
                self.entries
                    .iter()
                    .filter(|entry| entry.kind == EntryKind::Directory)
                    .map(|entry| entry.id.clone()),
            );
            self.expansion_initialized = true;
        }
    }

    pub fn schedule_preview(&mut self, source: String, delay: Duration) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        LATEST_PREVIEW_GENERATION.store(
            self.preview_generation,
            std::sync::atomic::Ordering::Relaxed,
        );
        self.pending_preview = Some(PendingPreview {
            generation: self.preview_generation,
            due: Instant::now() + delay,
            source,
        });
    }

    pub fn schedule_search(&mut self, query: String) {
        self.search_generation = self.search_generation.wrapping_add(1);
        LATEST_SEARCH_GENERATION
            .store(self.search_generation, std::sync::atomic::Ordering::Relaxed);
        if query.trim().is_empty() {
            self.pending_search = None;
            self.search_results.clear();
            return;
        }
        self.pending_search = Some(PendingSearch {
            generation: self.search_generation,
            due: Instant::now() + SEARCH_DEBOUNCE,
            query,
        });
    }

    pub fn schedule_scan(&mut self, delay: Duration) {
        self.schedule_scan_mode(delay, true);
    }

    pub fn schedule_current_file_check(&mut self, delay: Duration) {
        self.schedule_scan_mode(delay, false);
    }

    fn schedule_scan_mode(&mut self, delay: Duration, full_tree: bool) {
        if !self.workspace.is_open() {
            return;
        }
        self.scan_generation = self.scan_generation.wrapping_add(1);
        let due = Instant::now() + delay;
        self.pending_scan = Some(match self.pending_scan.take() {
            Some(pending) => PendingScan {
                generation: self.scan_generation,
                due: pending.due.min(due),
                full_tree: pending.full_tree || full_tree,
            },
            None => PendingScan {
                generation: self.scan_generation,
                due,
                full_tree,
            },
        });
    }

    pub fn entry(&self, id: &str) -> Option<&WorkspaceEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn selected_parent(&self) -> EntryId {
        if let Some(selected) = self.selected.as_deref() {
            if self
                .entry(selected)
                .is_some_and(|e| e.kind == EntryKind::Directory)
            {
                return selected.to_string();
            }
            return parent_id(selected);
        }
        self.current_file
            .as_deref()
            .map(parent_id)
            .unwrap_or_default()
    }

    pub fn current_is_under(&self, id: &str) -> bool {
        self.current_file
            .as_deref()
            .is_some_and(|current| current == id || current.starts_with(&format!("{id}/")))
    }

    pub fn delete_is_armed(&self, id: &str) -> bool {
        self.delete_armed
            .as_ref()
            .is_some_and(|(armed, when)| armed == id && when.elapsed() < Duration::from_secs(5))
    }
}

pub fn string_model(values: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        values
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

fn image_model(values: Vec<Image>) -> ModelRc<Image> {
    ModelRc::new(VecModel::from(values))
}
fn styled_model(values: Vec<StyledText>) -> ModelRc<StyledText> {
    ModelRc::new(VecModel::from(values))
}
fn int_model(values: Vec<i32>) -> ModelRc<i32> {
    ModelRc::new(VecModel::from(values))
}
fn bool_model(values: Vec<bool>) -> ModelRc<bool> {
    ModelRc::new(VecModel::from(values))
}

pub fn parent_id(id: &str) -> EntryId {
    id.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}

pub fn rebase_id(id: &str, old: &str, new: &str) -> EntryId {
    if id == old {
        return new.to_string();
    }
    id.strip_prefix(&format!("{old}/"))
        .map(|suffix| format!("{new}/{suffix}"))
        .unwrap_or_else(|| id.to_string())
}

pub fn set_status(ui: &MainWindow, text: impl Into<SharedString>) {
    ui.set_status(text.into());
}

pub fn sync_flags(ui: &MainWindow, state: &AppState) {
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    ui.set_app_build_number(
        option_env!("MARKERUP_BUILD_NUMBER")
            .unwrap_or("development")
            .into(),
    );
    ui.set_workspace_open(state.workspace.is_open());
    ui.set_workspace_is_smb(state.workspace.smb_config().is_some());
    ui.set_workspace_pinned(state.pinned);
    ui.set_workspace_path(state.workspace.display_name().into());
    ui.set_dirty(state.dirty);
    ui.set_external_conflict(state.external_conflict);
    ui.set_can_go_back(!state.back.is_empty());
    ui.set_can_go_forward(!state.forward.is_empty());
}

pub fn render_tree(ui: &MainWindow, state: &mut AppState) {
    let mut labels = Vec::new();
    let mut ids = Vec::new();
    // `entries` is emitted in depth-first order. Keep the currently visited
    // directory path instead of rebuilding every ancestor path for every row.
    let mut ancestors: Vec<EntryId> = Vec::new();
    for entry in &state.entries {
        ancestors.truncate(entry.depth);
        let visible = ancestors.iter().all(|id| state.expanded.contains(id));
        if visible {
            let indent = "    ".repeat(entry.depth);
            let marker = if state.selected.as_deref() == Some(entry.id.as_str()) {
                "• "
            } else {
                "  "
            };
            let kind = match entry.kind {
                EntryKind::Directory if state.expanded.contains(&entry.id) => "▼ ",
                EntryKind::Directory => "▶ ",
                EntryKind::File => "  ",
            };
            labels.push(format!("{marker}{indent}{kind}{}", entry.name));
            ids.push(entry.id.clone());
        }
        if entry.kind == EntryKind::Directory {
            ancestors.push(entry.id.clone());
        }
    }
    state.tree_ids = ids;
    state.tree_model.set_vec(
        labels
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    );
    ui.set_tree_labels(ModelRc::from(state.tree_model.clone()));
    ui.set_selected_path(state.selected.clone().unwrap_or_default().into());
}

fn styled_from_markdown(markdown: &str) -> StyledText {
    StyledText::from_markdown(markdown).unwrap_or_else(|_| StyledText::from_plain_text(markdown))
}

pub fn apply_preview_result(ui: &MainWindow, state: &mut AppState, result: PreviewResult) {
    if result.generation != state.preview_generation {
        return;
    }

    let apply_started = Instant::now();
    let mut texts = Vec::new();
    let mut plain_texts = Vec::new();
    let mut kinds = Vec::new();
    let mut heading_levels = Vec::new();
    let mut task_checked = Vec::new();

    for block in &result.blocks {
        plain_texts.push(if matches!(&block.kind, PreviewBlockKind::Mermaid) {
            String::new()
        } else {
            block.markdown.clone()
        });
        texts.push(
            if matches!(
                &block.kind,
                PreviewBlockKind::Mermaid | PreviewBlockKind::Image | PreviewBlockKind::Rule
            ) {
                StyledText::from_plain_text("")
            } else {
                styled_from_markdown(&block.markdown)
            },
        );
        match &block.kind {
            PreviewBlockKind::Body => {
                kinds.push(0);
                heading_levels.push(0);
                task_checked.push(false);
            }
            PreviewBlockKind::Heading(level) => {
                kinds.push(1);
                heading_levels.push(*level as i32);
                task_checked.push(false);
            }
            PreviewBlockKind::Task(checked) => {
                kinds.push(2);
                heading_levels.push(0);
                task_checked.push(*checked);
            }
            PreviewBlockKind::Mermaid => {
                kinds.push(3);
                heading_levels.push(0);
                task_checked.push(false);
            }
            PreviewBlockKind::Code => {
                kinds.push(4);
                heading_levels.push(0);
                task_checked.push(false);
            }
            PreviewBlockKind::List(ordered) => {
                kinds.push(if *ordered { 10 } else { 5 });
                heading_levels.push(0);
                task_checked.push(false);
            }
            PreviewBlockKind::Quote => {
                kinds.push(6);
                heading_levels.push(0);
                task_checked.push(false);
            }
            PreviewBlockKind::Rule => {
                kinds.push(7);
                heading_levels.push(0);
                task_checked.push(false);
            }
            PreviewBlockKind::Image => {
                kinds.push(8);
                heading_levels.push(0);
                task_checked.push(false);
            }
            PreviewBlockKind::Table => {
                kinds.push(9);
                heading_levels.push(0);
                task_checked.push(false);
            }
        }
    }

    ui.set_preview_block_texts(styled_model(texts));
    ui.set_preview_block_plain_texts(string_model(plain_texts));
    ui.set_preview_block_kinds(int_model(kinds));
    ui.set_preview_heading_levels(int_model(heading_levels));
    ui.set_preview_task_checked(bool_model(task_checked));

    let mut mermaid_images = Vec::with_capacity(result.blocks.len());
    let mut mermaid_errors = Vec::with_capacity(result.blocks.len());
    for (index, block) in result.blocks.iter().enumerate() {
        if !matches!(&block.kind, PreviewBlockKind::Mermaid) {
            mermaid_images.push(Image::default());
            mermaid_errors.push(String::new());
            continue;
        }
        match result.mermaid_svgs.get(index).and_then(|svg| svg.as_ref()) {
            Some(Ok(svg)) => match Image::load_from_svg_data(svg.as_bytes()) {
                Ok(image) => {
                    mermaid_images.push(image);
                    mermaid_errors.push(String::new());
                }
                Err(_) => {
                    mermaid_images.push(Image::default());
                    mermaid_errors.push("Mermaid SVG could not be displayed".to_string());
                }
            },
            Some(Err(error)) => {
                mermaid_images.push(Image::default());
                mermaid_errors.push(format!("Mermaid error: {error}"));
            }
            None => {
                mermaid_images.push(Image::default());
                mermaid_errors.push("Mermaid render result was missing".to_string());
            }
        }
    }
    ui.set_preview_block_mermaid_images(image_model(mermaid_images));
    ui.set_preview_block_mermaid_errors(string_model(mermaid_errors));

    let mut images = Vec::new();
    let mut labels = Vec::new();
    let mut block_images = vec![Image::default(); result.blocks.len()];
    if let Some(current) = state.current_file.as_deref() {
        let mut seen_assets = HashSet::new();
        for reference in result.images {
            let reference_destination = reference.destination.clone();
            let Some(asset_id) = state
                .workspace
                .resolve_asset_link(current, &reference.destination)
            else {
                continue;
            };
            if !seen_assets.insert(asset_id.clone()) {
                continue;
            }
            let Ok(Some(path)) = state.workspace.asset_path(&asset_id) else {
                continue;
            };
            let metadata = std::fs::metadata(&path).ok();
            let modified = metadata.as_ref().and_then(|value| value.modified().ok());
            let len = metadata.as_ref().map_or(0, |value| value.len());

            let image = state
                .image_cache
                .get(&asset_id)
                .filter(|cached| cached.modified == modified && cached.len == len)
                .map(|cached| cached.image.clone())
                .or_else(|| {
                    Image::load_from_path(&path).ok().map(|image| {
                        state.image_cache.insert(
                            asset_id.clone(),
                            CachedImage {
                                modified,
                                len,
                                image: image.clone(),
                            },
                        );
                        image
                    })
                });

            if let Some(image) = image {
                labels.push(if reference.alt.is_empty() {
                    asset_id
                } else {
                    reference.alt
                });
                images.push(image);
                if let Some((block_index, _)) =
                    result.blocks.iter().enumerate().find(|(_, block)| {
                        block.image.as_ref().is_some_and(|block_reference| {
                            block_reference.destination == reference_destination
                        })
                    })
                {
                    block_images[block_index] = images.last().cloned().unwrap_or_default();
                }
            }
        }
    }
    ui.set_preview_block_images(image_model(block_images));
    ui.set_preview_images(image_model(images));
    ui.set_preview_image_labels(string_model(labels));

    state.dirty = result.source_hash != state.saved_hash;
    sync_flags(ui, state);

    if perf_enabled() {
        eprintln!(
            "markerup perf: preview worker={:?} apply={:?}",
            result.elapsed,
            apply_started.elapsed()
        );
    }
}

pub fn apply_search_result(ui: &MainWindow, state: &mut AppState, result: SearchResult) {
    if result.generation != state.search_generation {
        return;
    }
    if result.cancelled {
        return;
    }
    match result.results {
        Ok(results) => {
            state.search_results = results.clone();
            ui.set_search_results(string_model(results));
        }
        Err(error) => set_status(ui, format!("Search failed: {error}")),
    }
    if perf_enabled() {
        eprintln!("markerup perf: search worker={:?}", result.elapsed);
    }
}

pub fn apply_scan_result(ui: &MainWindow, state: &mut AppState, result: ScanResult) {
    if result.generation != state.scan_generation {
        return;
    }
    let apply_started = Instant::now();

    let full_tree = result.entries.is_some();
    if let Some(entries) = result.entries {
        let entries = match entries {
            Ok(entries) => entries,
            Err(error) => {
                set_status(ui, format!("Workspace refresh failed: {error}"));
                return;
            }
        };
        let note_count = entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .count();
        state.apply_entries(entries);
        render_tree(ui, state);
        if note_count == 0 {
            set_status(ui, "Workspace loaded; no Markdown notes found");
        } else {
            set_status(
                ui,
                format!("Workspace loaded; {note_count} Markdown note(s) found"),
            );
        }
    }

    if let Some(current) = state.current_file.clone() {
        let still_exists = !full_tree
            || state
                .entries
                .iter()
                .any(|entry| entry.kind == EntryKind::File && entry.id == current);
        if !still_exists {
            if state.dirty {
                state.external_conflict = true;
                sync_flags(ui, state);
                set_status(ui, "CONFLICT: current note was deleted or moved externally");
            } else {
                clear_current(ui, state);
                set_status(ui, "Current note was removed externally");
            }
        } else if result.current_file.as_deref() == Some(current.as_str()) {
            match result.current_text {
                Some(Ok(disk)) if disk != state.disk_text => {
                    if state.dirty {
                        state.external_conflict = true;
                        sync_flags(ui, state);
                        set_status(
                            ui,
                            "CONFLICT: this note changed externally while you have unsaved edits",
                        );
                    } else {
                        state.disk_text = disk.clone();
                        state.saved_hash = hash_text(&disk);
                        set_editor_text(ui, disk.clone());
                        state.schedule_preview(disk, Duration::ZERO);
                        set_status(ui, "Reloaded external change");
                    }
                }
                Some(Err(error)) => set_status(ui, format!("External refresh failed: {error}")),
                _ => {}
            }
        }
    }

    if perf_enabled() {
        eprintln!(
            "markerup perf: workspace scan worker={:?} apply={:?}",
            result.elapsed,
            apply_started.elapsed()
        );
    }
}

fn clear_preview(ui: &MainWindow) {
    ui.set_preview_block_texts(styled_model(Vec::new()));
    ui.set_preview_block_plain_texts(string_model(Vec::new()));
    ui.set_preview_block_kinds(int_model(Vec::new()));
    ui.set_preview_heading_levels(int_model(Vec::new()));
    ui.set_preview_task_checked(bool_model(Vec::new()));
    ui.set_preview_block_mermaid_images(image_model(Vec::new()));
    ui.set_preview_block_mermaid_errors(string_model(Vec::new()));
    ui.set_preview_block_images(image_model(Vec::new()));
    ui.set_preview_images(image_model(Vec::new()));
    ui.set_preview_image_labels(string_model(Vec::new()));
}

pub fn save_session_for(state: &AppState) {
    if state.pinned {
        let root = state.workspace.root_path();
        let bookmark = state.workspace.bookmark();
        let smb = state.workspace.smb_config().map(|config| SavedSmbConfig {
            server: config.server,
            share: config.share,
            username: config.username,
            remote_path: config.remote_path,
        });
        let _ = save_session(
            root,
            state.current_file.as_deref(),
            bookmark.as_deref(),
            smb.as_ref(),
        );
        return;
    }
    let _ = clear_session();
}

pub fn clear_current(ui: &MainWindow, state: &mut AppState) {
    state.current_file = None;
    state.disk_text.clear();
    state.saved_hash = hash_text("");
    state.dirty = false;
    state.external_conflict = false;
    state.preview_generation = state.preview_generation.wrapping_add(1);
    state.pending_preview = None;
    ui.set_current_path("No note selected".into());
    set_editor_text(ui, String::new());
    clear_preview(ui);
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
        set_status(
            ui,
            "Unsaved changes: save or reload before leaving this note",
        );
        return false;
    }
    let contents = match state.workspace.read(&id) {
        Ok(contents) => contents,
        Err(error) => {
            set_status(ui, format!("Open failed: {error}"));
            return false;
        }
    };
    if history {
        if let Some(current) = state.current_file.clone() {
            if current != id {
                state.back.push(current);
                state.forward.clear();
            }
        }
    }
    state.current_file = Some(id.clone());
    state.selected = Some(id.clone());
    state.disk_text = contents.clone();
    state.saved_hash = hash_text(&contents);
    state.dirty = false;
    state.external_conflict = false;
    state.find_query.clear();
    state.find_matches.clear();
    state.find_index = 0;
    ui.set_current_path(id.into());
    set_editor_text(ui, contents.clone());
    ui.set_find_status("".into());
    clear_preview(ui);
    state.schedule_preview(contents, Duration::ZERO);
    render_tree(ui, state);
    sync_flags(ui, state);
    set_status(ui, "Ready");
    save_session_for(state);
    true
}

pub fn save_current(ui: &MainWindow, state: &mut AppState, contents: &str, force: bool) {
    let Some(current) = state.current_file.clone() else {
        set_status(ui, "No note selected");
        return;
    };
    if state.external_conflict && !force {
        set_status(
            ui,
            "File changed externally. Reload external changes or choose Overwrite.",
        );
        return;
    }
    match state.workspace.write(&current, contents) {
        Ok(()) => {
            state.disk_text = contents.to_string();
            state.saved_hash = hash_text(contents);
            state.dirty = false;
            state.external_conflict = false;
            sync_flags(ui, state);
            set_status(ui, format!("Saved {current}"));
        }
        Err(error) => set_status(ui, format!("Save failed: {error}")),
    }
}

pub fn reload_current(ui: &MainWindow, state: &mut AppState) {
    let Some(current) = state.current_file.clone() else {
        return;
    };
    match state.workspace.read(&current) {
        Ok(contents) => {
            state.disk_text = contents.clone();
            state.saved_hash = hash_text(&contents);
            state.dirty = false;
            state.external_conflict = false;
            set_editor_text(ui, contents.clone());
            state.schedule_preview(contents, Duration::ZERO);
            sync_flags(ui, state);
            set_status(ui, "Reloaded from disk");
        }
        Err(error) => set_status(ui, format!("Reload failed: {error}")),
    }
}

pub fn refresh_workspace(ui: &MainWindow, state: &mut AppState) {
    if !state.workspace.is_open() {
        return;
    }
    state.schedule_scan(Duration::ZERO);
    set_status(ui, "Refreshing workspace…");
}

pub fn mutate_refresh(ui: &MainWindow, state: &mut AppState) {
    state.schedule_scan(Duration::ZERO);
    render_tree(ui, state);
}

fn perf_enabled() -> bool {
    std::env::var_os("MARKERUP_PERF").is_some()
}
