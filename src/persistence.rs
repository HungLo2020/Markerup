use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SESSION_HEADER: &str = "markerup-session-v2";

#[derive(Debug, Clone)]
pub struct SessionState {
    pub pinned_workspace: PathBuf,
    pub current_file: Option<String>,
}

fn state_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("markerup/session"));
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config/markerup/session"))
}

pub fn load_session() -> Option<SessionState> {
    let text = fs::read_to_string(state_path()?).ok()?;
    let mut lines = text.lines();
    if lines.next()? != SESSION_HEADER { return None; }
    let pinned_workspace = PathBuf::from(lines.next()?);
    let current_file = lines.next().map(str::trim).filter(|line| !line.is_empty()).map(ToOwned::to_owned);
    Some(SessionState { pinned_workspace, current_file })
}

pub fn save_session(workspace: &Path, current_file: Option<&str>) -> io::Result<()> {
    let Some(path) = state_path() else { return Ok(()); };
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    fs::write(
        path,
        format!(
            "{SESSION_HEADER}\n{}\n{}\n",
            workspace.to_string_lossy(),
            current_file.unwrap_or("")
        ),
    )
}

pub fn clear_session() -> io::Result<()> {
    let Some(path) = state_path() else { return Ok(()); };
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
