#[cfg(target_os = "ios")]
mod ios_bridge;
#[cfg(target_os = "ios")]
mod ios_workspace;
mod markdown;
mod navigation;
mod persistence;
pub mod reminders;
mod smb_workspace;
mod tauri_backend;
mod workspace;

#[cfg(target_os = "linux")]
use tauri::Emitter;
use tauri::Manager;
use tauri_backend::MarkerupBackend;

#[cfg_attr(target_os = "ios", tauri::mobile_entry_point)]
pub fn run() {
    reminders::queue_navigation(&std::env::args().collect::<Vec<_>>());
    let builder = tauri::Builder::default();
    #[cfg(target_os = "linux")]
    let builder = builder.plugin(
        tauri_plugin_single_instance::Builder::new()
            .dbus_id(if cfg!(debug_assertions) {
                "com.matt.markerup.dev"
            } else {
                "com.matt.markerup"
            })
            .callback(|app, args, _| {
                reminders::queue_navigation(&args);
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                let _ = app.emit("reminder-activate", ());
            })
            .build(),
    );
    let backend = MarkerupBackend::default();
    let pending_restore = backend.load_saved_session();
    let builder = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(backend)
        .setup(move |app| {
            reminders::initialize(app.handle().clone());
            #[cfg(target_os = "ios")]
            ios_bridge::install_lifecycle_observers();
            if let Some(pending) = pending_restore {
                // Reopening a favorite can wait on SMB or a Files provider;
                // never hold up the first window (or the iOS launch watchdog).
                let app = app.handle().clone();
                std::thread::Builder::new()
                    .name("markerup-restore".to_string())
                    .spawn(move || app.state::<MarkerupBackend>().finish_restore(pending))?;
            }
            Ok(())
        });
    #[cfg(not(target_os = "ios"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        reminders::reminder_status,
        reminders::reminder_take_notification,
        reminders::reminder_permissions,
        reminders::reminder_rescan,
        reminders::reminder_validate,
        reminders::reminder_definition,
        reminders::reminder_definitions,
        reminders::reminder_workspace_action,
        reminders::reminder_target,
        tauri_backend::open_reminder_note,
        tauri_backend::workspace_snapshot,
        tauri_backend::restored_workspace,
        tauri_backend::open_local_workspace,
        tauri_backend::connect_smb,
        tauri_backend::open_note,
        tauri_backend::save_note,
        tauri_backend::reload_note,
        tauri_backend::refresh_workspace,
        tauri_backend::search_workspace,
        tauri_backend::workspace_assets,
        tauri_backend::create_note,
        tauri_backend::create_folder,
        tauri_backend::rename_entry,
        tauri_backend::move_entry,
        tauri_backend::delete_entry,
        tauri_backend::navigate_markdown_link,
        tauri_backend::go_back,
        tauri_backend::go_forward,
        tauri_backend::set_workspace_favorite,
        tauri_backend::open_favorite_workspace,
        tauri_backend::preview_document,
        tauri_backend::toggle_markdown_task,
        tauri_backend::render_mermaid,
        tauri_backend::workspace_asset_data,
        tauri_backend::privacy_policy_url
    ]);
    #[cfg(target_os = "ios")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        reminders::reminder_status,
        reminders::reminder_take_notification,
        reminders::reminder_permissions,
        reminders::reminder_rescan,
        reminders::reminder_validate,
        reminders::reminder_definition,
        reminders::reminder_definitions,
        reminders::reminder_workspace_action,
        reminders::reminder_target,
        tauri_backend::open_reminder_note,
        tauri_backend::workspace_snapshot,
        tauri_backend::restored_workspace,
        tauri_backend::open_local_workspace,
        tauri_backend::choose_ios_workspace,
        tauri_backend::connect_smb,
        tauri_backend::open_note,
        tauri_backend::save_note,
        tauri_backend::reload_note,
        tauri_backend::refresh_workspace,
        tauri_backend::search_workspace,
        tauri_backend::workspace_assets,
        tauri_backend::create_note,
        tauri_backend::create_folder,
        tauri_backend::rename_entry,
        tauri_backend::move_entry,
        tauri_backend::delete_entry,
        tauri_backend::navigate_markdown_link,
        tauri_backend::go_back,
        tauri_backend::go_forward,
        tauri_backend::set_workspace_favorite,
        tauri_backend::open_favorite_workspace,
        tauri_backend::preview_document,
        tauri_backend::toggle_markdown_task,
        tauri_backend::render_mermaid,
        tauri_backend::workspace_asset_data,
        tauri_backend::privacy_policy_url,
        tauri_backend::finish_ios_background_save
    ]);
    builder
        .build(tauri::generate_context!())
        .expect("Markerup Tauri application failed")
        .run(|_, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                reminders::flush_on_exit();
            }
        });
}
