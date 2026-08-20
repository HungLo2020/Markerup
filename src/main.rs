mod app;
mod handlers_editor;
mod handlers_workspace;
mod markdown;
mod persistence;
mod workers;
mod workspace;
mod workspace_picker;

use crate::app::{apply_preview_result, apply_scan_result, apply_search_result, open_file, render_tree, reset_workspace_ui, save_session_for, set_status, sync_flags, AppState, SCAN_DEBOUNCE};
use crate::persistence::{clear_session, load_session};
use crate::workers::{WorkerRequest, WorkerResult};
use crate::workspace::{LocalWorkspace, WorkspaceSlot};
use notify::{EventKind, RecursiveMode, Watcher};
use slint::{ComponentHandle, Timer, TimerMode};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

slint::include_modules!();

const UI_POLL_INTERVAL: Duration = Duration::from_millis(50);
const FULL_RECONCILE_INTERVAL_TICKS: u16 = 600; // 30 seconds at 50 ms/tick.

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
        render_tree(&ui, &mut state);
        sync_flags(&ui, &state);
        if state.workspace.is_open() {
            state.schedule_scan(Duration::ZERO);
        }
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

            let generations = {
                let state = state.borrow();
                (state.preview_generation, state.search_generation, state.scan_generation)
            };
            {
                let mut state = state.borrow_mut();
                state.replace_workspace(WorkspaceSlot::local(workspace), false);
                state.preview_generation = generations.0.wrapping_add(1);
                state.search_generation = generations.1.wrapping_add(1);
                state.scan_generation = generations.2.wrapping_add(1);
                reset_workspace_ui(&ui, &mut state);
                state.schedule_scan(Duration::ZERO);
                sync_flags(&ui, &state);
            }

            if let Some(error) = watch_error {
                set_status(&ui, format!("Workspace selected; automatic file watching unavailable: {error}"));
            } else {
                set_status(&ui, "Workspace selected. Loading notes in the background…");
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

    let (workers, worker_rx) = workers::spawn_workers();
    let timer = Timer::default();
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let reconcile_tick = Cell::new(0u16);
        timer.start(TimerMode::Repeated, UI_POLL_INTERVAL, move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let now = Instant::now();

            let mut watcher_requires_full_scan = false;
            let mut watcher_current_file_changed = false;
            let mut watcher_error = None;
            while let Ok(result) = watch_rx.try_recv() {
                match result {
                    Ok(event) => {
                        // Do not react to access/read notifications. The
                        // workspace scanner itself opens directories while
                        // walking them, and Linux notify backends can report
                        // those opens as Access events. Treating them as
                        // mutations creates a scan -> access event -> scan
                        // loop and invalidates every scan result before it can
                        // reach the UI.
                        if matches!(event.kind, EventKind::Access(_)) {
                            continue;
                        }

                        let current_file_path = {
                            let state = state.borrow();
                            state.workspace.root_path().and_then(|root| {
                                state.current_file.as_deref().map(|id| root.join(id))
                            })
                        };
                        let is_current_file_modify = matches!(event.kind, EventKind::Modify(_))
                            && current_file_path.as_ref().is_some_and(|current| {
                                !event.paths.is_empty() && event.paths.iter().all(|path| path == current)
                            });
                        if is_current_file_modify {
                            watcher_current_file_changed = true;
                        } else {
                            watcher_requires_full_scan = true;
                        }
                    }
                    Err(error) => watcher_error = Some(error.to_string()),
                }
            }
            if let Some(error) = watcher_error {
                set_status(&ui, format!("File watcher error: {error}"));
            }

            let next_tick = reconcile_tick.get().wrapping_add(1);
            reconcile_tick.set(next_tick);
            let periodic_reconcile = next_tick % FULL_RECONCILE_INTERVAL_TICKS == 0;

            {
                let mut state = state.borrow_mut();
                if state.workspace.is_open() && (watcher_requires_full_scan || periodic_reconcile) {
                    // Filesystem watchers commonly emit several events for one
                    // logical save. Coalesce them before walking the workspace.
                    state.schedule_scan(SCAN_DEBOUNCE);
                } else if state.workspace.is_open() && watcher_current_file_changed {
                    state.schedule_current_file_check(SCAN_DEBOUNCE);
                }

                if ui.get_view_mode() != 0 {
                    let ready = state.pending_preview.as_ref().is_some_and(|pending| pending.due <= now);
                    if ready {
                        if let Some(pending) = state.pending_preview.take() {
                            if workers.preview.send(WorkerRequest::Preview {
                                generation: pending.generation,
                                source: pending.source,
                            }).is_err() {
                                set_status(&ui, "Preview worker stopped unexpectedly");
                            }
                        }
                    }
                }

                let search_ready = state.pending_search.as_ref().is_some_and(|pending| pending.due <= now);
                if search_ready {
                    if let Some(pending) = state.pending_search.take() {
                        if let Some(workspace) = state.workspace.local_clone() {
                            if workers.io.send(WorkerRequest::Search {
                                generation: pending.generation,
                                workspace,
                                query: pending.query,
                            }).is_err() {
                                set_status(&ui, "Search worker stopped unexpectedly");
                            }
                        }
                    }
                }

                let scan_ready = state.pending_scan.as_ref().is_some_and(|pending| pending.due <= now);
                if scan_ready {
                    if let Some(pending) = state.pending_scan.take() {
                        if let Some(workspace) = state.workspace.local_clone() {
                            if workers.io.send(WorkerRequest::Scan {
                                generation: pending.generation,
                                workspace,
                                current_file: state.current_file.clone(),
                                full_tree: pending.full_tree,
                            }).is_err() {
                                set_status(&ui, "Workspace worker stopped unexpectedly");
                            }
                        }
                    }
                }
            }

            while let Ok(result) = worker_rx.try_recv() {
                let mut state = state.borrow_mut();
                match result {
                    WorkerResult::Preview(result) => apply_preview_result(&ui, &mut state, result),
                    WorkerResult::Search(result) => apply_search_result(&ui, &mut state, result),
                    WorkerResult::Scan(result) => apply_scan_result(&ui, &mut state, result),
                }
            }
        });
    }

    if let Some(restored) = restore_file {
        // `open_file` performs the authoritative read. Avoid reading the
        // restored note once just to check whether it exists and then again
        // to display it.
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
