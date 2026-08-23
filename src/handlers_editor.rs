use crate::MainWindow;
use crate::app::{
    AUTOSAVE_DEBOUNCE, AppState, PREVIEW_DEBOUNCE, open_file, refresh_workspace, reload_current,
    save_current, set_status, string_model, sync_flags,
};
use crate::markdown::toggle_task_at_offset;
use crate::markdown::{find_heading_range, find_matches};
use crate::workspace::Workspace;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

pub fn wire(ui: &MainWindow, state: Rc<RefCell<AppState>>) {
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_preview_task_toggled(move |index| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            if state.current_file.is_none() || state.external_conflict {
                return;
            }
            let source = ui.get_editor_text().to_string();
            if index < 0 {
                set_status(&ui, "Could not locate the Markdown task");
                return;
            }
            let Some(updated) = toggle_task_at_offset(&source, index as usize) else {
                set_status(&ui, "Could not locate the Markdown task");
                return;
            };
            ui.set_editor_text(updated.clone().into());
            state.dirty = true;
            state.schedule_preview(updated, PREVIEW_DEBOUNCE);
            state.schedule_autosave(ui.get_editor_text().to_string(), AUTOSAVE_DEBOUNCE);
            sync_flags(&ui, &state);
            set_status(&ui, "Unsaved changes");
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_overwrite_requested(move |contents| {
            if let Some(ui) = ui_weak.upgrade() {
                save_current(&ui, &mut state.borrow_mut(), &contents, true);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_reload_requested(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = state.borrow_mut();
                if state.dirty {
                    let contents = ui.get_editor_text().to_string();
                    save_current(&ui, &mut state, &contents, false);
                    if state.dirty {
                        return;
                    }
                }
                reload_current(&ui, &mut state);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_editor_changed(move |contents| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            if state.current_file.is_none() {
                return;
            }
            state.dirty = true;
            state.schedule_preview(contents.to_string(), PREVIEW_DEBOUNCE);
            state.schedule_autosave(contents.to_string(), AUTOSAVE_DEBOUNCE);
            sync_flags(&ui, &state);
            set_status(&ui, "Unsaved changes");
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_refresh_requested(move || {
            if let Some(ui) = ui_weak.upgrade() {
                refresh_workspace(&ui, &mut state.borrow_mut());
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_search_requested(move |query| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            state.schedule_search(query.to_string());
            if query.trim().is_empty() {
                ui.set_search_results(string_model(Vec::new()));
            } else {
                set_status(&ui, "Searching…");
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_search_result_selected(move |index| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let id = state.borrow().search_results.get(index as usize).cloned();
            if let Some(id) = id {
                open_file(&ui, &mut state.borrow_mut(), id, true);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_find_next_requested(move |query| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let source = ui.get_editor_text().to_string();
            let mut state = state.borrow_mut();
            let query = query.to_string();
            if state.find_query != query {
                state.find_query = query.clone();
                state.find_matches = find_matches(&source, &query);
                state.find_index = 0;
            } else if !state.find_matches.is_empty() {
                state.find_index = (state.find_index + 1) % state.find_matches.len();
            }
            if state.find_matches.is_empty() {
                ui.set_find_status("No matches".into());
                return;
            }
            let (start, end) = state.find_matches[state.find_index];
            ui.invoke_select_editor_range(start as i32, end as i32);
            ui.set_find_status(
                format!("{}/{}", state.find_index + 1, state.find_matches.len()).into(),
            );
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_preview_link_clicked(move |link| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(current) = state.borrow().current_file.clone() else {
                return;
            };
            let target = state
                .borrow()
                .workspace
                .resolve_markdown_link(&current, &link);
            let Some(target) = target else {
                if state
                    .borrow()
                    .workspace
                    .resolve_asset_link(&current, &link)
                    .is_some()
                {
                    set_status(&ui, format!("Local asset: {link}"));
                } else {
                    set_status(&ui, format!("Not a local Markdown link: {link}"));
                }
                return;
            };

            let anchor = target.anchor.clone();
            if open_file(&ui, &mut state.borrow_mut(), target.id, true)
                && let Some(anchor) = anchor
            {
                let source = ui.get_editor_text().to_string();
                if let Some((start, end)) = find_heading_range(&source, &anchor) {
                    ui.invoke_select_editor_range(start as i32, end as i32);
                    set_status(&ui, format!("Opened #{anchor}"));
                } else {
                    set_status(&ui, format!("Opened note; heading #{anchor} was not found"));
                }
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_back_requested(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let target = {
                let mut state = state.borrow_mut();
                let current = state.current_file.clone();
                state.navigation.go_back(current.as_deref())
            };
            if let Some(target) = target {
                open_file(&ui, &mut state.borrow_mut(), target, false);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_forward_requested(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let target = {
                let mut state = state.borrow_mut();
                let current = state.current_file.clone();
                state.navigation.go_forward(current.as_deref())
            };
            if let Some(target) = target {
                open_file(&ui, &mut state.borrow_mut(), target, false);
            }
        });
    }
}
