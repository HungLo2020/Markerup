mod markdown;
mod workspace;

use crate::markdown::preview_markdown;
use crate::workspace::{LocalWorkspace, Workspace};
use slint::{ComponentHandle, ModelRc, SharedString, StyledText, VecModel};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

slint::include_modules!();

struct AppState {
    workspace: LocalWorkspace,
    files: Vec<PathBuf>,
    current_file: Option<PathBuf>,
}

impl AppState {
    fn new(workspace: LocalWorkspace) -> Self {
        Self {
            workspace,
            files: Vec::new(),
            current_file: None,
        }
    }

    fn refresh_files(&mut self) -> std::io::Result<()> {
        self.files = self.workspace.markdown_files()?;
        Ok(())
    }
}

fn string_model(values: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        values
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

fn set_preview(ui: &MainWindow, source: &str) {
    let compatible = preview_markdown(source);
    let styled = StyledText::from_markdown(&compatible)
        .unwrap_or_else(|_| StyledText::from_plain_text(source));
    ui.set_preview(styled);
}

fn show_file(ui: &MainWindow, state: &mut AppState, relative_path: PathBuf) {
    match state.workspace.read(&relative_path) {
        Ok(contents) => {
            state.current_file = Some(relative_path.clone());
            ui.set_current_path(relative_path.to_string_lossy().into());
            ui.set_editor_text(contents.clone().into());
            set_preview(ui, &contents);
            ui.set_status("Ready".into());
        }
        Err(error) => ui.set_status(format!("Open failed: {error}").into()),
    }
}

fn refresh_file_list(ui: &MainWindow, state: &mut AppState) {
    match state.refresh_files() {
        Ok(()) => {
            ui.set_files(string_model(
                state
                    .files
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned()),
            ));
            ui.set_status(
                format!(
                    "{} Markdown file{}",
                    state.files.len(),
                    if state.files.len() == 1 { "" } else { "s" }
                )
                .into(),
            );
        }
        Err(error) => ui.set_status(format!("Refresh failed: {error}").into()),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let workspace = LocalWorkspace::open(&root)?;

    let ui = MainWindow::new()?;
    ui.set_workspace_path(workspace.root().to_string_lossy().into());

    let state = Rc::new(RefCell::new(AppState::new(workspace)));
    refresh_file_list(&ui, &mut state.borrow_mut());

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_file_selected(move |index| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let path = state.borrow().files.get(index as usize).cloned();
            if let Some(path) = path {
                show_file(&ui, &mut state.borrow_mut(), path);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_save_requested(move |contents| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let current = state.borrow().current_file.clone();
            let Some(current) = current else {
                ui.set_status("No note selected".into());
                return;
            };

            match state.borrow().workspace.write(&current, &contents) {
                Ok(()) => ui.set_status(format!("Saved {}", current.display()).into()),
                Err(error) => ui.set_status(format!("Save failed: {error}").into()),
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        ui.on_editor_changed(move |contents| {
            if let Some(ui) = ui_weak.upgrade() {
                set_preview(&ui, &contents);
                ui.set_status("Modified (not saved)".into());
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_preview_link_clicked(move |link| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let current = state.borrow().current_file.clone();
            let Some(current) = current else { return };
            let target = state
                .borrow()
                .workspace
                .resolve_markdown_link(current.as_path(), &link);

            if let Some(target) = target {
                show_file(&ui, &mut state.borrow_mut(), target);
            } else {
                ui.set_status(format!("Not a local Markdown link: {link}").into());
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_refresh_requested(move || {
            if let Some(ui) = ui_weak.upgrade() {
                refresh_file_list(&ui, &mut state.borrow_mut());
            }
        });
    }

    let first = state.borrow().files.first().cloned();
    if let Some(first) = first {
        show_file(&ui, &mut state.borrow_mut(), first);
    }

    ui.run()?;
    Ok(())
}
