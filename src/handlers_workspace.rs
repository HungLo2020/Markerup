use crate::app::{mutate_refresh, open_file, rebase_id, render_tree, save_session_for, set_status, AppState};
use crate::workspace::{EntryKind, Workspace};
use crate::MainWindow;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

pub fn wire(ui: &MainWindow, state: Rc<RefCell<AppState>>) {
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_tree_selected(move |index| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let id = state.borrow().tree_ids.get(index as usize).cloned();
            let Some(id) = id else { return };
            let kind = state.borrow().entry(&id).map(|entry| entry.kind);
            match kind {
                Some(EntryKind::Directory) => {
                    let mut state = state.borrow_mut();
                    state.selected = Some(id.clone());
                    if !state.expanded.remove(&id) { state.expanded.insert(id); }
                    render_tree(&ui, &mut state);
                }
                Some(EntryKind::File) => { open_file(&ui, &mut state.borrow_mut(), id, true); }
                None => {}
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_new_note_requested(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let parent = state.borrow().selected_parent();
            let result = { let s = state.borrow(); s.workspace.create_note(&parent, &name) };
            match result {
                Ok(id) => {
                    let mut state = state.borrow_mut();
                    if !parent.is_empty() { state.expanded.insert(parent); }
                    mutate_refresh(&ui, &mut state);
                    open_file(&ui, &mut state, id, true);
                    ui.set_action_name("".into());
                }
                Err(error) => set_status(&ui, format!("Create note failed: {error}")),
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_new_folder_requested(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let parent = state.borrow().selected_parent();
            let result = { let s = state.borrow(); s.workspace.create_directory(&parent, &name) };
            match result {
                Ok(id) => {
                    let mut state = state.borrow_mut();
                    state.expanded.insert(id.clone());
                    if !parent.is_empty() { state.expanded.insert(parent); }
                    state.selected = Some(id);
                    mutate_refresh(&ui, &mut state);
                    ui.set_action_name("".into());
                    set_status(&ui, "Folder created");
                }
                Err(error) => set_status(&ui, format!("Create folder failed: {error}")),
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_rename_requested(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(old) = state.borrow().selected.clone() else {
                set_status(&ui, "Select a note or folder to rename"); return;
            };
            if state.borrow().dirty && state.borrow().current_is_under(&old) {
                set_status(&ui, "Save or reload the current note before renaming it"); return;
            }
            let result = { let s = state.borrow(); s.workspace.rename(&old, &name) };
            match result {
                Ok(new) => {
                    let mut state = state.borrow_mut();
                    state.selected = state.selected.as_deref().map(|id| rebase_id(id, &old, &new));
                    state.current_file = state.current_file.as_deref().map(|id| rebase_id(id, &old, &new));
                    state.back = state.back.iter().map(|id| rebase_id(id, &old, &new)).collect();
                    state.forward = state.forward.iter().map(|id| rebase_id(id, &old, &new)).collect();
                    state.expanded = state.expanded.iter().map(|id| rebase_id(id, &old, &new)).collect();
                    mutate_refresh(&ui, &mut state);
                    if let Some(current) = state.current_file.clone() { ui.set_current_path(current.into()); }
                    save_session_for(&state);
                    ui.set_action_name("".into());
                    set_status(&ui, "Renamed");
                }
                Err(error) => set_status(&ui, format!("Rename failed: {error}")),
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_delete_requested(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(id) = state.borrow().selected.clone() else {
                set_status(&ui, "Select a note or folder to delete"); return;
            };
            if state.borrow().dirty && state.borrow().current_is_under(&id) {
                set_status(&ui, "Save or reload the current note before deleting it"); return;
            }
            if !state.borrow().delete_is_armed(&id) {
                state.borrow_mut().delete_armed = Some((id.clone(), Instant::now()));
                set_status(&ui, format!("Press Delete again within 5 seconds to delete {id}"));
                return;
            }
            let result = { let s = state.borrow(); s.workspace.delete(&id) };
            match result {
                Ok(()) => {
                    let mut state = state.borrow_mut();
                    state.delete_armed = None;
                    if state.current_is_under(&id) { crate::app::clear_current(&ui, &mut state); }
                    state.selected = None;
                    state.back.retain(|v| v != &id && !v.starts_with(&format!("{id}/")));
                    state.forward.retain(|v| v != &id && !v.starts_with(&format!("{id}/")));
                    mutate_refresh(&ui, &mut state);
                    set_status(&ui, "Deleted");
                }
                Err(error) => set_status(&ui, format!("Delete failed: {error}")),
            }
        });
    }
}
