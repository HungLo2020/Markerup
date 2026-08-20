use std::path::PathBuf;

#[cfg(target_os = "linux")]
pub fn choose_workspace() -> Result<Option<PathBuf>, String> {
    Ok(rfd::FileDialog::new()
        .set_title("Choose Markerup workspace")
        .pick_folder())
}

#[cfg(not(target_os = "linux"))]
pub fn choose_workspace() -> Result<Option<PathBuf>, String> {
    Err("workspace picker is not implemented for this platform yet".to_string())
}
