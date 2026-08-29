#[cfg(target_os = "ios")]
mod ios_bridge;
#[cfg(target_os = "ios")]
mod ios_workspace;
mod markdown;
mod navigation;
mod persistence;
mod smb_workspace;
mod tauri_backend;
mod workspace;

use tauri_backend::MarkerupBackend;

#[cfg_attr(target_os = "ios", tauri::mobile_entry_point)]
pub fn run() {
    let backend = MarkerupBackend::default();
    backend.restore();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(backend);
    #[cfg(not(target_os = "ios"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        tauri_backend::workspace_snapshot,
        tauri_backend::open_local_workspace,
        tauri_backend::connect_smb,
        tauri_backend::open_note,
        tauri_backend::save_note,
        tauri_backend::reload_note,
        tauri_backend::refresh_workspace,
        tauri_backend::search_workspace,
        tauri_backend::create_note,
        tauri_backend::create_folder,
        tauri_backend::rename_entry,
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
        tauri_backend::workspace_snapshot,
        tauri_backend::open_local_workspace,
        tauri_backend::choose_ios_workspace,
        tauri_backend::connect_smb,
        tauri_backend::open_note,
        tauri_backend::save_note,
        tauri_backend::reload_note,
        tauri_backend::refresh_workspace,
        tauri_backend::search_workspace,
        tauri_backend::create_note,
        tauri_backend::create_folder,
        tauri_backend::rename_entry,
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
    builder
        .run(tauri::generate_context!())
        .expect("Markerup Tauri application failed");
}
