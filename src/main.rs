mod app;
mod handlers_editor;
mod handlers_workspace;
mod markdown;
mod persistence;
mod workspace;
mod workspace_picker;

use crate::app::{open_file, refresh_workspace, render_tree, reset_workspace_ui, save_session_for, set_status, sync_flags, AppState};
use crate::persistence::{clear_session, load_session};
use crate::workspace::{LocalWorkspace, Workspace, WorkspaceSlot};
use notify::{RecursiveMode, Watcher};
use slint::{ComponentHandle, Timer, TimerMode};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let saved = load_session();
    let (workspace, restore_file, pinned) = match saved {
        Some(session) if session.pinned_workspace.is_dir() => {
            match LocalWorkspace::open(&session.pinned_workspace) {
                Ok(workspace) => (WorkspaceSlot::local(workspace), session.current_file, true),
                Err(_) => {
                    let _ = clear_session();
                    (WorkspaceSlot::Empty, None, false)
                }
            }
        }
        Some(_) => {
            let _ = clear_session();
            (WorkspaceSlot::Empty, None, false)
        }
        None => (WorkspaceSlot::Empty, None, false),
    };

    let ui = MainWindow::new()?;
    let state = Rc::new(RefCell::new(AppState::new(workspace, pinned)));
    {
        let mut state = state.borrow_mut();
        state.refresh_entries()?;
        render_tree(&ui, &mut state);
        sync_flags(&ui, &state);
    }

    let (watch_tx, watch_rx) = mpsc::channel();
    let watcher = Rc::new(RefCell::new(notify::recommended_watcher(move |result| {
        let _ = watch_tx.send(result);
    })?));

    if let Some(root) = state.borrow().workspace.root_path() {
        if let Err(error) = watcher.borrow_mut().watch(root, RecursiveMode::Recursive) {
            set_status(&ui, format!("Workspace opened, but file watching failed: {error}"));
        }
    }

    handlers_workspace::wire(&ui, state.clone());
    handlers_editor::wire(&ui, state.clone());

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let watcher = watcher.clone();
        ui.on_choose_workspace_requested(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if state.borrow().dirty {
                set_status(&ui, "Save or reload the current note before changing workspaces");
                return;
            }

            let chosen = match workspace_picker::choose_workspace() {
                Ok(Some(path)) => path,
                Ok(None) => return,
                Err(error) => {
                    set_status(&ui, format!("Folder picker failed: {error}"));
                    return;
                }
            };

            let workspace = match LocalWorkspace::open(&chosen) {
                Ok(workspace) => workspace,
                Err(error) => {
                    set_status(&ui, format!("Could not open workspace: {error}"));
                    return;
                }
            };

            let old_root = state.borrow().workspace.root_path().map(ToOwned::to_owned);
            if let Some(old_root) = old_root.as_deref() {
                let _ = watcher.borrow_mut().unwatch(old_root);
            }
            let new_root = workspace.root_path().to_path_buf();
            let watch_error = watcher.borrow_mut().watch(&new_root, RecursiveMode::Recursive).err();

            {
                let mut state = state.borrow_mut();
                state.replace_workspace(WorkspaceSlot::local(workspace), false);
                if let Err(error) = state.refresh_entries() {
                    set_status(&ui, format!("Workspace scan failed: {error}"));
                    return;
                }
                reset_workspace_ui(&ui, &mut state);
                sync_flags(&ui, &state);
            }

            if let Some(error) = watch_error {
                set_status(&ui, format!("Workspace selected; automatic file watching unavailable: {error}"));
            } else {
                set_status(&ui, "Workspace selected. Pin it to reopen automatically next time.");
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_pin_workspace_requested(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            if !state.workspace.is_open() { return; }
            state.pinned = !state.pinned;
            save_session_for(&state);
            sync_flags(&ui, &state);
            set_status(
                &ui,
                if state.pinned {
                    "Workspace pinned; it will reopen on next launch"
                } else {
                    "Workspace unpinned; it will stay open only for this session"
                },
            );
        });
    }

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
            if state.borrow().workspace.is_open() && (changed || periodic_reconcile) {
                refresh_workspace(&ui, &mut state.borrow_mut());
            }
        });
    }

    if let Some(restored) = restore_file.filter(|id| state.borrow().workspace.read(id).is_ok()) {
        open_file(&ui, &mut state.borrow_mut(), restored, false);
    } else if !state.borrow().workspace.is_open() {
        set_status(&ui, "Choose a folder to use as a Markerup workspace");
    }
    sync_flags(&ui, &state.borrow());

    ui.run()?;
    drop(timer);
    drop(watcher);
    Ok(())
}
