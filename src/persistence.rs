use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

const SESSION_HEADER: &str = "markerup-session-v4";
const PREVIOUS_SESSION_HEADER: &str = "markerup-session-v3";
const LEGACY_SESSION_HEADER: &str = "markerup-session-v2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSmbConfig {
    pub server: String,
    pub share: String,
    pub username: String,
    pub remote_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SavedWorkspace {
    Local {
        path: PathBuf,
        bookmark: Option<Vec<u8>>,
    },
    Smb(SavedSmbConfig),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedFavorite {
    pub workspace: SavedWorkspace,
}

#[derive(Debug, Clone)]
pub struct SessionState {
    pub favorites: Vec<SavedFavorite>,
    pub active_favorite: Option<usize>,
    pub current_file: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSession {
    favorites: Vec<SavedFavorite>,
    active_favorite: Option<usize>,
    current_file: Option<String>,
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
    let (header, payload) = text.split_once('\n')?;
    if header == SESSION_HEADER {
        let stored: StoredSession = serde_json::from_str(payload).ok()?;
        let active_favorite = stored
            .active_favorite
            .filter(|index| *index < stored.favorites.len());
        return Some(SessionState {
            favorites: stored.favorites,
            active_favorite,
            current_file: stored.current_file,
        });
    }
    load_legacy_session(header, payload)
}

pub fn save_session(
    favorites: &[SavedFavorite],
    active_favorite: Option<usize>,
    current_file: Option<&str>,
) -> io::Result<()> {
    let Some(path) = state_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let stored = StoredSession {
        favorites: favorites.to_vec(),
        active_favorite: active_favorite.filter(|index| *index < favorites.len()),
        current_file: current_file.map(ToOwned::to_owned),
    };
    let payload = serde_json::to_string(&stored).map_err(|error| {
        io::Error::other(format!("could not encode Markerup favorites: {error}"))
    })?;
    fs::write(path, format!("{SESSION_HEADER}\n{payload}\n"))
}

fn load_legacy_session(header: &str, payload: &str) -> Option<SessionState> {
    if header != PREVIOUS_SESSION_HEADER && header != LEGACY_SESSION_HEADER {
        return None;
    }
    let mut lines = payload.lines();
    let pinned_workspace = PathBuf::from(lines.next()?);
    let current_file = lines
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned);
    let bookmark = lines.next().and_then(decode_hex);
    let smb = if header == PREVIOUS_SESSION_HEADER {
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
    let workspace = smb
        .map(SavedWorkspace::Smb)
        .unwrap_or(SavedWorkspace::Local {
            path: pinned_workspace,
            bookmark,
        });
    Some(SessionState {
        favorites: vec![SavedFavorite { workspace }],
        active_favorite: Some(0),
        current_file,
    })
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).ok())
        .collect()
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

#[cfg(test)]
mod tests {
    use super::{
        LEGACY_SESSION_HEADER, SavedFavorite, SavedSmbConfig, SavedWorkspace, StoredSession,
        load_legacy_session,
    };
    use std::path::PathBuf;

    #[test]
    fn favorites_round_trip_without_smb_passwords() {
        let stored = StoredSession {
            favorites: vec![
                SavedFavorite {
                    workspace: SavedWorkspace::Local {
                        path: PathBuf::from("/notes"),
                        bookmark: Some(vec![1, 2, 3]),
                    },
                },
                SavedFavorite {
                    workspace: SavedWorkspace::Smb(SavedSmbConfig {
                        server: "nas.local".to_string(),
                        share: "notes".to_string(),
                        username: "matt".to_string(),
                        remote_path: "Markdown".to_string(),
                    }),
                },
            ],
            active_favorite: Some(1),
            current_file: Some("Inbox.md".to_string()),
        };
        let encoded = serde_json::to_string(&stored).unwrap();
        assert!(!encoded.contains("password"));
        let decoded: StoredSession = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.favorites.len(), 2);
        assert_eq!(decoded.active_favorite, Some(1));
    }

    #[test]
    fn legacy_pinned_workspace_becomes_an_active_favorite() {
        let restored = load_legacy_session(LEGACY_SESSION_HEADER, "/notes\nInbox.md\n\n")
            .expect("legacy session should remain readable");
        assert_eq!(restored.active_favorite, Some(0));
        assert_eq!(restored.current_file.as_deref(), Some("Inbox.md"));
        assert_eq!(
            restored.favorites,
            vec![SavedFavorite {
                workspace: SavedWorkspace::Local {
                    path: PathBuf::from("/notes"),
                    bookmark: None,
                },
            }]
        );
    }
}
