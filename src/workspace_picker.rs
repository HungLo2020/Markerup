#[cfg(not(target_os = "ios"))]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
pub fn choose_workspace() -> Result<Option<PathBuf>, String> {
    Ok(rfd::FileDialog::new()
        .set_title("Choose Markerup workspace")
        .pick_folder())
}

#[cfg(all(not(target_os = "linux"), not(target_os = "ios")))]
pub fn choose_workspace() -> Result<Option<PathBuf>, String> {
    Err("workspace picker is not implemented for this platform yet".to_string())
}

#[cfg(target_os = "ios")]
pub use crate::ios_bridge::WorkspaceSelection;

#[cfg(target_os = "ios")]
pub fn choose_workspace(
    callback: impl FnOnce(Result<Option<WorkspaceSelection>, String>) + 'static,
) {
    crate::ios_bridge::choose_workspace(callback);
}
