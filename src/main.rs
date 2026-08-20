mod app;
mod handlers_editor;
mod handlers_workspace;
mod markdown;
mod persistence;
mod workspace;

use crate::app::{open_file, refresh_workspace, render_tree, set_status, sync_flags, AppState};
use crate::persistence::load_session;
use crate::workspace::{EntryKind, LocalWorkspace, Workspace};
use notify::{RecursiveMode, Watcher};
use slint::{ComponentHandle, Timer, TimerMode};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let saved = load_session();
    let explicit_root = std::env::args_os().nth(1).map(PathBuf::from);
    let root = explicit_root
        .clone()
        .or_else(|| saved.as_ref().map(|session| session.workspace.clone()))
        .filter(|path| path.is_dir())
        .unwrap_or(std::env::current_dir()?);

    let workspace = LocalWorkspace::open(&root)?;
    let watcher_root = workspace.root_path().to_path_buf();
    let restore_file = explicit_root
        .is_none()
        .then(|| saved.as_ref().and_then(|session| session.current_file.clone()))
        .flatten();

    let ui = MainWindow::new()?;
    ui.set_workspace_path(workspace.root_display().into());

    let state = Rc::new(RefCell::new(AppState::new(workspace)));
    {
        let mut state = state.borrow_mut();
        state.refresh_entries()?;
        render_tree(&ui, &mut state);
    }

    handlers_workspace::wire(&ui, state.clone());
    handlers_editor::wire(&ui, state.clone());

    let (watch_tx, watch_rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |result| {
        let _ = watch_tx.send(result);
    })?;
    watcher.watch(&watcher_root, RecursiveMode::Recursive)?;

    let timer = Timer::default();
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let polling_tick = Cell::new(0u8);
        timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
            let mut changed = false;
            let mut last_error = None;
            while let Ok(result) = watch_rx.try_recv() {
                match result {
                    Ok(_) => changed = true,
                    Err(error) => last_error = Some(error.to_string()),
                }
            }

            let next_tick = polling_tick.get().wrapping_add(1);
            polling_tick.set(next_tick);
            let periodic_reconcile = next_tick % 4 == 0;

            let Some(ui) = ui_weak.upgrade() else { return };
            if let Some(error) = last_error {
                set_status(&ui, format!("File watcher error: {error}"));
            }
            if changed || periodic_reconcile {
                refresh_workspace(&ui, &mut state.borrow_mut());
            }
        });
    }

    let restored = restore_file
        .filter(|id| state.borrow().workspace.read(id).is_ok())
        .or_else(|| {
            state
                .borrow()
                .entries
                .iter()
                .find(|entry| entry.kind == EntryKind::File)
                .map(|entry| entry.id.clone())
        });
    if let Some(restored) = restored {
        open_file(&ui, &mut state.borrow_mut(), restored, false);
    }
    sync_flags(&ui, &state.borrow());

    ui.run()?;
    drop(timer);
    drop(watcher);
    Ok(())
}
