use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SESSION_HEADER: &str = "markerup-session-v3";
const LEGACY_SESSION_HEADER: &str = "markerup-session-v2";

#[derive(Debug, Clone)]
pub struct SavedSmbConfig {
    pub server: String,
    pub share: String,
    pub username: String,
    pub remote_path: String,
}

#[derive(Debug, Clone)]
pub struct SessionState {
    #[allow(dead_code)]
    pub pinned_workspace: PathBuf,
    pub current_file: Option<String>,
    #[allow(dead_code)]
    pub bookmark: Option<Vec<u8>>,
    #[allow(dead_code)]
    pub smb: Option<SavedSmbConfig>,
}

fn state_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("markerup/session"));
    }
    std::env::var_os("HOME").map(PathBuf::from).map(|home| {
        if cfg!(target_os = "ios") {
            home.join("Library/Application Support/Markerup/session")
        } else {
            home.join(".config/markerup/session")
        }
    })
}

pub fn load_session() -> Option<SessionState> {
    let text = fs::read_to_string(state_path()?).ok()?;
    let mut lines = text.lines();
    let header = lines.next()?;
    if header != SESSION_HEADER && header != LEGACY_SESSION_HEADER {
        return None;
    }
    let pinned_workspace = PathBuf::from(lines.next()?);
    let current_file = lines
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned);
    let bookmark = lines.next().and_then(decode_hex);
    let smb = if header == SESSION_HEADER {
        let server = decode_text(lines.next()?)?;
        let share = decode_text(lines.next()?)?;
        let username = decode_text(lines.next()?)?;
        let remote_path = decode_text(lines.next()?)?;
        if server.is_empty() && share.is_empty() && username.is_empty() && remote_path.is_empty() {
            None
        } else {
            Some(SavedSmbConfig {
                server,
                share,
                username,
                remote_path,
            })
        }
    } else {
        None
    };
    Some(SessionState {
        pinned_workspace,
        current_file,
        bookmark,
        smb,
    })
}

pub fn save_session(
    workspace: Option<&Path>,
    current_file: Option<&str>,
    bookmark: Option<&[u8]>,
    smb: Option<&SavedSmbConfig>,
) -> io::Result<()> {
    let Some(path) = state_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        format!(
            "{SESSION_HEADER}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            workspace.map_or(String::new(), |path| path.to_string_lossy().into_owned()),
            current_file.unwrap_or(""),
            bookmark.map(encode_hex).unwrap_or_default(),
            smb.map_or(String::new(), |value| encode_text(&value.server)),
            smb.map_or(String::new(), |value| encode_text(&value.share)),
            smb.map_or(String::new(), |value| encode_text(&value.username)),
            smb.map_or(String::new(), |value| encode_text(&value.remote_path)),
        ),
    )
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).ok())
        .collect()
}

fn encode_text(text: &str) -> String {
    encode_hex(text.as_bytes())
}

fn decode_text(text: &str) -> Option<String> {
    if text.is_empty() {
        return Some(String::new());
    }
    String::from_utf8(decode_hex(text)?).ok()
}

pub fn clear_session() -> io::Result<()> {
    let Some(path) = state_path() else {
        return Ok(());
    };
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
