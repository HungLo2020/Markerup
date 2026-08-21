use crate::workspace::{EntryId, EntryKind, LinkTarget, LocalWorkspace, Workspace, WorkspaceEntry};
use smb2::{ClientConfig, DirectoryEntry, SmbClient, Tree};
use std::future::Future;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const SMB_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const SMB_SCAN_TIMEOUT: Duration = Duration::from_secs(120);

/// Connection information for a direct SMB workspace.
///
/// The password is intentionally kept only in memory by this type. Callers
/// decide whether and how to persist it through a platform secure-secret
/// store; it must never be serialized into workspace/session files.
#[derive(Clone)]
pub struct SmbConnectionConfig {
    pub server: String,
    pub share: String,
    pub username: String,
    pub password: String,
    pub remote_path: String,
}

impl SmbConnectionConfig {
    pub fn validate(&self) -> io::Result<()> {
        if self.server.trim().is_empty() || self.share.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SMB server and share are required",
            ));
        }
        validate_remote_path(&self.remote_path)?;
        Ok(())
    }

    fn address(&self) -> String {
        if self.server.contains(':') {
            self.server.clone()
        } else {
            format!("{}:445", self.server)
        }
    }
}

struct SmbSession {
    client: SmbClient,
    tree: Tree,
}

/// A filesystem-first workspace backed directly by an SMB2/3 share.
///
/// The backend owns one reconnectable SMB session and serializes operations
/// through a mutex. Workspace methods are synchronous because the existing
/// application abstraction is synchronous; callers use it from worker
/// threads, never from the UI event thread.
pub struct SmbWorkspace {
    config: SmbConnectionConfig,
    runtime: tokio::runtime::Runtime,
    session: Mutex<SmbSession>,
}

impl SmbWorkspace {
    pub fn connect(config: SmbConnectionConfig) -> io::Result<Self> {
        config.validate()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|error| io::Error::other(format!("could not create SMB runtime: {error}")))?;
        let session = runtime.block_on(async { connect_session(&config).await })?;
        Ok(Self {
            config,
            runtime,
            session: Mutex::new(session),
        })
    }

    fn remote_path(&self, id: &str) -> io::Result<String> {
        validate_remote_id(id)?;
        let root = normalize_remote_path(&self.config.remote_path)?;
        if id.is_empty() {
            return Ok(root);
        }
        Ok(if root.is_empty() {
            id.replace('\\', "/")
        } else {
            format!("{root}/{}", id.replace('\\', "/"))
        })
    }

    fn reconnect(&self, session: &mut SmbSession) -> io::Result<()> {
        *session = self
            .runtime
            .block_on(async { connect_session(&self.config).await })?;
        Ok(())
    }

    fn run_smb<T, F, Fut>(&self, operation: &'static str, future: F) -> io::Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, smb2::Error>>,
    {
        match self.runtime.block_on(async move {
            // Construct the smb2 future inside the runtime. Some smb2
            // operations query Tokio's reactor while they are built.
            tokio::time::timeout(SMB_OPERATION_TIMEOUT, future()).await
        }) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(io::Error::other(format!("SMB {operation} failed: {error}"))),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "SMB {operation} timed out after {} seconds",
                    SMB_OPERATION_TIMEOUT.as_secs()
                ),
            )),
        }
    }

    fn list_directory(&self, path: &str) -> io::Result<Vec<DirectoryEntry>> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| io::Error::other("SMB session lock poisoned"))?;
        let result = {
            let SmbSession { client, tree } = &mut *session;
            self.run_smb("directory enumeration", || {
                client.list_directory(tree, path)
            })
        };
        match result {
            Ok(entries) => Ok(entries),
            Err(first) => {
                self.reconnect(&mut session).map_err(|reconnect| {
                    io::Error::other(format!(
                        "SMB list failed: {first}; reconnect failed: {reconnect}"
                    ))
                })?;
                let SmbSession { client, tree } = &mut *session;
                self.run_smb("directory enumeration after reconnect", || {
                    client.list_directory(tree, path)
                })
                .map_err(|retry| {
                    io::Error::other(format!("SMB list failed after reconnect: {retry}"))
                })
            }
        }
    }

    fn read_remote(&self, path: &str) -> io::Result<Vec<u8>> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| io::Error::other("SMB session lock poisoned"))?;
        let result = {
            let SmbSession { client, tree } = &mut *session;
            self.run_smb("read", || client.read_file_pipelined(tree, path))
        };
        match result {
            Ok(data) => Ok(data),
            Err(first) => {
                self.reconnect(&mut session).map_err(|reconnect| {
                    io::Error::other(format!(
                        "SMB read failed: {first}; reconnect failed: {reconnect}"
                    ))
                })?;
                let SmbSession { client, tree } = &mut *session;
                self.run_smb("read after reconnect", || {
                    client.read_file_pipelined(tree, path)
                })
                .map_err(|retry| {
                    io::Error::other(format!("SMB read failed after reconnect: {retry}"))
                })
            }
        }
    }

    fn mutate<T>(
        &self,
        operation: impl FnOnce(&mut SmbClient, &mut Tree) -> io::Result<T>,
    ) -> io::Result<T> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| io::Error::other("SMB session lock poisoned"))?;
        let SmbSession { client, tree } = &mut *session;
        operation(client, tree)
    }

    fn collect_entries(
        &self,
        directory: &str,
        relative_directory: &str,
        depth: usize,
        deadline: Instant,
        output: &mut Vec<WorkspaceEntry>,
    ) -> io::Result<()> {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "SMB workspace scan timed out",
            ));
        }
        let mut children = self.list_directory(directory)?;
        children.sort_by_key(|entry| entry.name.to_lowercase());
        for child in children {
            if child.name == "." || child.name == ".." || child.name.starts_with('.') {
                continue;
            }
            let id = if relative_directory.is_empty() {
                child.name.clone()
            } else {
                format!("{relative_directory}/{}", child.name)
            };
            if child.is_directory {
                let child_name = child.name.clone();
                output.push(WorkspaceEntry {
                    id: id.clone(),
                    name: child.name,
                    kind: EntryKind::Directory,
                    depth,
                });
                let child_directory = if directory.is_empty() {
                    child_name
                } else {
                    format!("{directory}/{child_name}")
                };
                self.collect_entries(&child_directory, &id, depth + 1, deadline, output)?;
            } else if child.name.to_ascii_lowercase().ends_with(".md") {
                output.push(WorkspaceEntry {
                    id,
                    name: child.name,
                    kind: EntryKind::File,
                    depth,
                });
            }
        }
        Ok(())
    }

    fn resolve_relative(&self, current_file: &str, link: &str) -> Option<String> {
        let (path, fragment) = link.split_once('#').unwrap_or((link, ""));
        let path = path.split('?').next()?.trim();
        if path.is_empty()
            || path.starts_with('/')
            || path.contains("://")
            || path.starts_with("mailto:")
        {
            return None;
        }
        validate_remote_id(current_file).ok()?;
        let parent = Path::new(current_file)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        let candidate = parent.join(path);
        let mut components = Vec::new();
        for component in candidate.components() {
            match component {
                Component::Normal(value) => components.push(value.to_string_lossy().into_owned()),
                Component::CurDir => {}
                Component::ParentDir => {
                    components.pop()?;
                }
                _ => return None,
            }
        }
        let id = components.join("/");
        Some(format!("{id}\0{fragment}"))
    }
}

async fn connect_session(config: &SmbConnectionConfig) -> io::Result<SmbSession> {
    let client = SmbClient::connect(ClientConfig {
        addr: config.address(),
        timeout: Duration::from_secs(15),
        username: config.username.clone(),
        password: config.password.clone(),
        domain: String::new(),
        auto_reconnect: true,
        compression: true,
        dfs_enabled: true,
        dfs_target_overrides: Default::default(),
    })
    .await
    .map_err(|error| io::Error::other(format!("SMB connection/authentication failed: {error}")))?;
    let mut client = client;
    let tree = client
        .connect_share(config.share.trim())
        .await
        .map_err(|error| io::Error::other(format!("SMB share connection failed: {error}")))?;
    Ok(SmbSession { client, tree })
}

fn normalize_remote_path(path: &str) -> io::Result<String> {
    let raw = path.trim();
    if raw.starts_with('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SMB folder path must be relative to the share",
        ));
    }
    let path = raw.trim_matches('/').replace('\\', "/");
    validate_remote_path(&path)?;
    Ok(path)
}

fn validate_remote_path(path: &str) -> io::Result<()> {
    if path.starts_with('/') || path.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SMB folder path must be relative to the share",
        ));
    }
    for component in path.replace('\\', "/").split('/') {
        if component == ".." {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SMB folder path cannot escape the share",
            ));
        }
        if component.is_empty() {
            continue;
        }
        if component == "." {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SMB folder path cannot contain '.' components",
            ));
        }
    }
    Ok(())
}

fn validate_remote_id(id: &str) -> io::Result<()> {
    if id.is_empty() {
        return Ok(());
    }
    // IDs are generated with `/` separators. Reject `\\` explicitly instead
    // of relying on the host OS's Path parser; otherwise a Windows-style
    // `a\\..\\b` could become an escaping path on the SMB server.
    if id.starts_with('/') || id.contains('\\') || id.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SMB entry escapes workspace",
        ));
    }
    let path = Path::new(id);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::CurDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SMB entry escapes workspace",
        ));
    }
    Ok(())
}

impl Workspace for SmbWorkspace {
    fn entries(&self) -> io::Result<Vec<WorkspaceEntry>> {
        let root = self.remote_path("")?;
        let mut entries = Vec::new();
        self.collect_entries(
            &root,
            "",
            0,
            Instant::now() + SMB_SCAN_TIMEOUT,
            &mut entries,
        )?;
        Ok(entries)
    }

    fn entries_with_cancel(
        &self,
        should_cancel: &dyn Fn() -> bool,
    ) -> io::Result<Option<Vec<WorkspaceEntry>>> {
        if should_cancel() {
            return Ok(None);
        }
        let entries = self.entries()?;
        Ok((!should_cancel()).then_some(entries))
    }

    fn markdown_files(&self) -> io::Result<Vec<EntryId>> {
        Ok(self
            .entries()?
            .into_iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .map(|entry| entry.id)
            .collect())
    }

    fn read(&self, id: &str) -> io::Result<String> {
        let data = self.read_remote(&self.remote_path(id)?)?;
        String::from_utf8(data).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn write(&self, id: &str, contents: &str) -> io::Result<()> {
        let path = self.remote_path(id)?;
        self.mutate(|client, tree| {
            self.run_smb("write", || {
                client.write_file(tree, &path, contents.as_bytes())
            })
            .map(|_| ())
        })
    }

    fn create_note(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        let mut name = LocalWorkspace::validate_name(name)?.to_string();
        if !name.to_ascii_lowercase().ends_with(".md") {
            name.push_str(".md");
        }
        let id = if parent.is_empty() {
            name
        } else {
            format!("{parent}/{name}")
        };
        if self.entries()?.iter().any(|entry| entry.id == id) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "an SMB entry with that name already exists",
            ));
        }
        self.write(
            &id,
            &format!(
                "# {}\n",
                Path::new(&id)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
            ),
        )?;
        Ok(id)
    }

    fn create_directory(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        validate_remote_id(parent)?;
        let name = LocalWorkspace::validate_name(name)?;
        let id = if parent.is_empty() {
            name.to_string()
        } else {
            format!("{parent}/{name}")
        };
        let path = self.remote_path(&id)?;
        self.mutate(|client, tree| {
            self.run_smb("directory creation", || {
                client.create_directory(tree, &path)
            })
        })?;
        Ok(id)
    }

    fn rename(&self, id: &str, new_name: &str) -> io::Result<EntryId> {
        validate_remote_id(id)?;
        let name = LocalWorkspace::validate_name(new_name)?;
        let source = self.remote_path(id)?;
        let parent = Path::new(id).parent().unwrap_or_else(|| Path::new(""));
        let destination_id = if parent.as_os_str().is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", parent.to_string_lossy().replace('\\', "/"), name)
        };
        let destination = self.remote_path(&destination_id)?;
        self.mutate(|client, tree| {
            self.run_smb("rename", || client.rename(tree, &source, &destination))
        })?;
        Ok(destination_id)
    }

    fn delete(&self, id: &str) -> io::Result<()> {
        let path = self.remote_path(id)?;
        let is_directory = self.list_directory(&path).is_ok();
        self.mutate(|client, tree| {
            let result = if is_directory {
                self.run_smb("directory delete", || client.delete_directory(tree, &path))
            } else {
                self.run_smb("file delete", || client.delete_file(tree, &path))
            };
            result
        })
    }

    fn search_markdown(&self, query: &str) -> io::Result<Vec<EntryId>> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut results = Vec::new();
        for id in self.markdown_files()? {
            if id.to_lowercase().contains(&query)
                || self
                    .read(&id)
                    .map(|text| text.to_lowercase().contains(&query))
                    .unwrap_or(false)
            {
                results.push(id);
            }
        }
        Ok(results)
    }

    fn resolve_markdown_link(&self, current_file: &str, link: &str) -> Option<LinkTarget> {
        let resolved = self.resolve_relative(current_file, link)?;
        let (id, fragment) = resolved.split_once('\0')?;
        id.to_ascii_lowercase()
            .ends_with(".md")
            .then(|| LinkTarget {
                id: id.to_string(),
                anchor: (!fragment.is_empty()).then(|| fragment.to_string()),
            })
    }

    fn resolve_asset_link(&self, current_file: &str, link: &str) -> Option<EntryId> {
        self.resolve_relative(current_file, link)
            .and_then(|value| value.split_once('\0').map(|(id, _)| id.to_string()))
    }
    fn display_name(&self) -> String {
        format!("SMB {} / {}", self.config.server, self.config.share)
    }
    fn identity(&self) -> String {
        format!(
            "smb:{}:{}:{}",
            self.config.server, self.config.share, self.config.remote_path
        )
    }
    fn asset_path(&self, _id: &str) -> io::Result<Option<PathBuf>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::{SmbConnectionConfig, SmbWorkspace, normalize_remote_path, validate_remote_id};
    use crate::workspace::Workspace;

    #[test]
    fn validates_smb_connection_paths() {
        assert_eq!(
            normalize_remote_path(" Documents/Notes/ ").unwrap(),
            "Documents/Notes"
        );
        assert!(normalize_remote_path("../outside").is_err());
        assert!(normalize_remote_path("/absolute").is_err());
        assert!(validate_remote_id("nested/../outside").is_err());
        assert!(validate_remote_id(r"nested\..\outside").is_err());
        assert!(validate_remote_id("./nested/Note.md").is_err());
        assert!(validate_remote_id("nested/Note.md").is_ok());
    }

    #[test]
    fn validates_required_connection_fields_without_connecting() {
        let config = SmbConnectionConfig {
            server: "server.example".into(),
            share: "Documents".into(),
            username: "user".into(),
            password: "secret".into(),
            remote_path: "Notes".into(),
        };
        assert!(config.validate().is_ok());
        assert!(
            SmbConnectionConfig {
                server: String::new(),
                ..config
            }
            .validate()
            .is_err()
        );
    }

    /// Opt-in Linux validation against a real SMB2/3 server. Credentials are
    /// read only from the process environment and are never printed. Run with
    /// `cargo test real_smb_round_trip -- --ignored --nocapture` after setting
    /// the MARKERUP_SMB_* variables documented in docs/smb-workspaces.md.
    #[test]
    #[ignore = "requires an explicitly configured real SMB server"]
    fn real_smb_round_trip() {
        let config = SmbConnectionConfig {
            server: std::env::var("MARKERUP_SMB_SERVER").expect("MARKERUP_SMB_SERVER is required"),
            share: std::env::var("MARKERUP_SMB_SHARE").expect("MARKERUP_SMB_SHARE is required"),
            username: std::env::var("MARKERUP_SMB_USERNAME").unwrap_or_default(),
            password: std::env::var("MARKERUP_SMB_PASSWORD").unwrap_or_default(),
            remote_path: std::env::var("MARKERUP_SMB_REMOTE_PATH").unwrap_or_default(),
        };
        let workspace = SmbWorkspace::connect(config).expect("SMB connection failed");
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let directory = format!("markerup-smb-test-{suffix}");
        let note = format!("{directory}/round-trip.md");
        let renamed = format!("{directory}/renamed.md");

        workspace
            .create_directory("", &directory)
            .expect("create directory failed");
        let result = (|| {
            workspace
                .write(&note, "# SMB round trip\n")
                .expect("write failed");
            assert_eq!(
                workspace.read(&note).expect("read failed"),
                "# SMB round trip\n"
            );
            assert!(
                workspace
                    .markdown_files()
                    .expect("enumeration failed")
                    .contains(&note)
            );
            assert_eq!(
                workspace
                    .rename(&note, "renamed.md")
                    .expect("rename failed"),
                renamed
            );
            workspace.delete(&renamed).expect("delete failed");
            Ok::<(), ()>(())
        })();
        let _ = workspace.delete(&directory);
        result.expect("SMB round trip failed");
    }
}
