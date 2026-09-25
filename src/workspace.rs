use crate::smb_workspace::SmbWorkspace;
use percent_encoding::percent_decode_str;
use serde::Serialize;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const NANOSECONDS_PER_SECOND: u128 = 1_000_000_000;
const SECONDS_PER_DAY: u128 = 86_400;
const BACKUP_ROOT: &str = ".markerup/backups";
const RECENT_SAVE_COUNT: usize = 5;
const DAILY_RETENTION_DAYS: i64 = 5;
const MONTHLY_RETENTION_MONTHS: i64 = 5;

pub(crate) fn unix_time_nanos() -> io::Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos())
}

pub(crate) fn recovery_suffix() -> io::Result<String> {
    Ok(format!("{:020}-{}", unix_time_nanos()?, std::process::id()))
}

pub(crate) fn backup_directory_id(id: &str) -> io::Result<String> {
    let relative = LocalWorkspace::validate_id(id)?;
    let filename = relative
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "note has no name"))?;
    let mut directory = PathBuf::from(BACKUP_ROOT);
    if let Some(parent) = relative.parent() {
        directory.push(parent);
    }
    directory.push(filename);
    Ok(directory.to_string_lossy().replace('\\', "/"))
}

pub(crate) fn backup_snapshot_name(suffix: &str) -> String {
    format!("{suffix}.md")
}

fn backup_timestamp(name: &str) -> Option<u128> {
    let suffix = name.strip_suffix(".md")?;
    let (timestamp, _process_id) = suffix.split_once('-')?;
    timestamp.parse().ok()
}

pub(crate) fn is_backup_snapshot_name(name: &str) -> bool {
    backup_timestamp(name).is_some()
}

fn month_serial_from_days(days_since_epoch: i64) -> i64 {
    // Howard Hinnant's civil-from-days conversion, returning a zero-based
    // month serial so adjacent months can be selected by subtraction.
    let shifted_days = days_since_epoch + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days
    } else {
        shifted_days - 146_096
    } / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    year * 12 + month - 1
}

fn timestamp_day(timestamp: u128) -> i64 {
    (timestamp / NANOSECONDS_PER_SECOND / SECONDS_PER_DAY) as i64
}

fn timestamp_month(timestamp: u128) -> i64 {
    month_serial_from_days(timestamp_day(timestamp))
}

pub(crate) fn retained_backup_names(names: &[String], now_nanos: u128) -> HashSet<String> {
    let mut snapshots: Vec<(u128, &String)> = names
        .iter()
        .filter_map(|name| backup_timestamp(name).map(|timestamp| (timestamp, name)))
        .collect();
    snapshots.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));

    let mut retained: HashSet<String> = snapshots
        .iter()
        .rev()
        .take(RECENT_SAVE_COUNT)
        .map(|(_, name)| (*name).clone())
        .collect();

    let latest_for = |matches: &dyn Fn(u128) -> bool| {
        snapshots
            .iter()
            .rev()
            .find(|(timestamp, _)| matches(*timestamp))
            .map(|(_, name)| (*name).clone())
    };

    let current_day = timestamp_day(now_nanos);
    for days_ago in 0..DAILY_RETENTION_DAYS {
        let day = current_day - days_ago;
        if let Some(name) = latest_for(&|timestamp| timestamp_day(timestamp) == day) {
            retained.insert(name);
        }
    }

    let current_month = timestamp_month(now_nanos);
    for months_ago in 0..MONTHLY_RETENTION_MONTHS {
        let month = current_month - months_ago;
        if let Some(name) = latest_for(&|timestamp| timestamp_month(timestamp) == month) {
            retained.insert(name);
        }
    }

    retained
}

fn prune_local_backups(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .collect();
    let names: Vec<_> = files
        .iter()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    let Ok(now_nanos) = unix_time_nanos() else {
        return;
    };
    let retained = retained_backup_names(&names, now_nanos);
    for entry in files {
        let name = entry.file_name().to_string_lossy().into_owned();
        if backup_timestamp(&name).is_some() && !retained.contains(&name) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn ensure_local_directory(root: &Path, relative: &str) -> io::Result<PathBuf> {
    let relative = LocalWorkspace::validate_id(relative)?;
    let mut directory = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "backup directory escapes the workspace",
            ));
        };
        directory.push(name);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "backup directory cannot contain symbolic links",
                ));
            }
            Ok(metadata) if metadata.is_dir() => (),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "backup directory path conflicts with a file",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&directory)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(directory)
}

#[cfg(target_os = "ios")]
use crate::ios_workspace::IosWorkspace;

pub type EntryId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EntryKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceEntry {
    pub id: EntryId,
    pub name: String,
    pub kind: EntryKind,
    pub depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkTarget {
    pub id: EntryId,
    pub anchor: Option<String>,
}

#[allow(dead_code)] // Kept for provider cancellation and identity-aware backends.
pub trait Workspace: Send + Sync {
    fn entries(&self) -> io::Result<Vec<WorkspaceEntry>>;
    fn asset_entries(&self) -> io::Result<Vec<WorkspaceEntry>>;
    fn entries_with_cancel(
        &self,
        should_cancel: &dyn Fn() -> bool,
    ) -> io::Result<Option<Vec<WorkspaceEntry>>>;
    #[allow(dead_code)]
    fn markdown_files(&self) -> io::Result<Vec<EntryId>>;
    fn read(&self, id: &str) -> io::Result<String>;
    fn write(&self, id: &str, contents: &str) -> io::Result<()>;
    fn create_note(&self, parent: &str, name: &str) -> io::Result<EntryId>;
    fn create_directory(&self, parent: &str, name: &str) -> io::Result<EntryId>;
    fn rename(&self, id: &str, new_name: &str) -> io::Result<EntryId>;
    fn move_entry(&self, id: &str, destination_parent: &str) -> io::Result<EntryId>;
    fn delete(&self, id: &str) -> io::Result<()>;
    #[allow(dead_code)]
    fn search_markdown(&self, query: &str) -> io::Result<Vec<EntryId>>;
    fn resolve_markdown_link(&self, current_file: &str, link: &str) -> Option<LinkTarget>;
    fn resolve_asset_link(&self, current_file: &str, link: &str) -> Option<EntryId>;
    fn display_name(&self) -> String;
    fn identity(&self) -> String;
    fn asset_path(&self, id: &str) -> io::Result<Option<PathBuf>>;
    fn asset_bytes(&self, id: &str) -> io::Result<Vec<u8>>;
}

#[allow(dead_code)]
pub type WorkspaceRef = Arc<dyn Workspace>;

#[derive(Debug, Clone)]
pub struct LocalWorkspace {
    root: PathBuf,
}

impl LocalWorkspace {
    #[cfg(not(target_os = "ios"))]
    pub fn open(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace path is not a directory",
            ));
        }
        Ok(Self { root })
    }

    pub fn root_path(&self) -> &Path {
        &self.root
    }
    pub fn root_display(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }
    pub fn absolute_asset_path(&self, id: &str) -> io::Result<PathBuf> {
        self.absolute_existing(id)
    }

    pub(crate) fn validate_id(id: &str) -> io::Result<PathBuf> {
        let path = Path::new(id);
        if path.is_absolute()
            || path.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "entry escapes the workspace",
            ));
        }
        Ok(path.to_path_buf())
    }

    pub(crate) fn validate_name(name: &str) -> io::Result<&str> {
        let name = name.trim();
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "name must be a single non-empty path component",
            ));
        }
        Ok(name)
    }

    pub(crate) fn absolute_existing(&self, id: &str) -> io::Result<PathBuf> {
        let relative = Self::validate_id(id)?;
        let canonical = fs::canonicalize(self.root.join(relative))?;
        if !canonical.starts_with(&self.root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "entry escapes the workspace",
            ));
        }
        Ok(canonical)
    }

    /// Build a path beneath a security-scoped iOS workspace without asking
    /// the local filesystem to canonicalize a File Provider/SMB URL.
    #[cfg(target_os = "ios")]
    pub(crate) fn scoped_path(&self, id: &str) -> io::Result<PathBuf> {
        Ok(self.root.join(Self::validate_id(id)?))
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn open_scoped(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        if root.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace path is empty",
            ));
        }
        Ok(Self { root })
    }

    pub(crate) fn absolute_parent(&self, parent: &str) -> io::Result<PathBuf> {
        if parent.is_empty() {
            return Ok(self.root.clone());
        }
        let path = self.absolute_existing(parent)?;
        if !path.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "parent is not a directory",
            ));
        }
        Ok(path)
    }

    pub(crate) fn id_for_path(&self, path: &Path) -> io::Result<EntryId> {
        let relative = path.strip_prefix(&self.root).map_err(|_| {
            io::Error::new(io::ErrorKind::PermissionDenied, "entry escapes workspace")
        })?;
        Ok(relative.to_string_lossy().replace('\\', "/"))
    }

    fn scan_dir(
        &self,
        directory: &Path,
        depth: usize,
        entries: &mut Vec<WorkspaceEntry>,
        should_cancel: Option<&dyn Fn() -> bool>,
    ) -> io::Result<bool> {
        let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
        for child in children {
            if should_cancel.is_some_and(|cancel| cancel()) {
                return Ok(false);
            }
            let name = child.file_name();
            let name_text = name.to_string_lossy();
            if name_text.starts_with('.') {
                continue;
            }

            let path = child.path();
            let ty = child.file_type()?;
            if ty.is_dir() {
                entries.push(WorkspaceEntry {
                    id: self.id_for_path(&path)?,
                    name: name_text.into_owned(),
                    kind: EntryKind::Directory,
                    depth,
                });
                if !self.scan_dir(&path, depth + 1, entries, should_cancel)? {
                    return Ok(false);
                }
            } else if ty.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                entries.push(WorkspaceEntry {
                    id: self.id_for_path(&path)?,
                    name: name_text.into_owned(),
                    kind: EntryKind::File,
                    depth,
                });
            }
        }
        Ok(true)
    }

    fn scan_assets(
        &self,
        directory: &Path,
        depth: usize,
        entries: &mut Vec<WorkspaceEntry>,
    ) -> io::Result<()> {
        let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
        for child in children {
            let name = child.file_name();
            let name_text = name.to_string_lossy();
            if name_text.starts_with('.') {
                continue;
            }
            let path = child.path();
            let ty = child.file_type()?;
            if ty.is_dir() {
                self.scan_assets(&path, depth + 1, entries)?;
            } else if ty.is_file() && is_supported_asset(&path) {
                entries.push(WorkspaceEntry {
                    id: self.id_for_path(&path)?,
                    name: name_text.into_owned(),
                    kind: EntryKind::File,
                    depth,
                });
            }
        }
        Ok(())
    }

    pub fn entries_with_cancel<F>(
        &self,
        should_cancel: F,
    ) -> io::Result<Option<Vec<WorkspaceEntry>>>
    where
        F: Fn() -> bool,
    {
        let mut entries = Vec::new();
        if self.scan_dir(&self.root, 0, &mut entries, Some(&should_cancel))? {
            Ok(Some(entries))
        } else {
            Ok(None)
        }
    }

    fn resolved_relative_link(&self, current_file: &str, raw: &str) -> Option<PathBuf> {
        let decoded = percent_decode_str(raw).decode_utf8().ok()?;
        let target = decoded.trim();
        if target.is_empty()
            || target.starts_with('/')
            || target.contains("://")
            || target.starts_with("mailto:")
        {
            return None;
        }
        let current = Self::validate_id(current_file).ok()?;
        let parent = current.parent().unwrap_or_else(|| Path::new(""));
        let canonical = fs::canonicalize(self.root.join(parent).join(target)).ok()?;
        canonical.starts_with(&self.root).then_some(canonical)
    }
}

impl Workspace for LocalWorkspace {
    fn entries(&self) -> io::Result<Vec<WorkspaceEntry>> {
        let mut entries = Vec::new();
        self.scan_dir(&self.root, 0, &mut entries, None)?;
        Ok(entries)
    }

    fn asset_entries(&self) -> io::Result<Vec<WorkspaceEntry>> {
        let mut entries = Vec::new();
        self.scan_assets(&self.root, 0, &mut entries)?;
        Ok(entries)
    }

    fn entries_with_cancel(
        &self,
        should_cancel: &dyn Fn() -> bool,
    ) -> io::Result<Option<Vec<WorkspaceEntry>>> {
        let mut entries = Vec::new();
        if self.scan_dir(&self.root, 0, &mut entries, Some(should_cancel))? {
            Ok(Some(entries))
        } else {
            Ok(None)
        }
    }

    fn markdown_files(&self) -> io::Result<Vec<EntryId>> {
        Ok(self
            .entries()?
            .into_iter()
            .filter(|e| e.kind == EntryKind::File)
            .map(|e| e.id)
            .collect())
    }

    fn read(&self, id: &str) -> io::Result<String> {
        fs::read_to_string(self.absolute_existing(id)?)
    }

    fn write(&self, id: &str, contents: &str) -> io::Result<()> {
        let path = self.absolute_existing(id)?;
        if !path.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "entry is not a file",
            ));
        }
        let backup_directory = ensure_local_directory(&self.root, &backup_directory_id(id)?)?;
        let backup = backup_directory.join(backup_snapshot_name(&recovery_suffix()?));
        fs::copy(&path, &backup)?;
        fs::File::open(&backup)?.sync_all()?;
        fs::File::open(&backup_directory)?.sync_all()?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("note has no parent"))?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("note has no name"))?
            .to_string_lossy();
        let temporary = parent.join(format!(".{name}.markerup-writing-{}", recovery_suffix()?));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.set_permissions(fs::metadata(&path)?.permissions())?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, &path)?;
            // The replacement is atomic; syncing the directory also makes its
            // new directory entry durable across an abrupt power loss.
            fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        prune_local_backups(&backup_directory);
        Ok(())
    }

    fn create_note(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        let mut name = Self::validate_name(name)?.to_string();
        if !name.to_ascii_lowercase().ends_with(".md") {
            name.push_str(".md");
        }
        let path = self.absolute_parent(parent)?.join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let title = path.file_stem().unwrap_or_default().to_string_lossy();
        writeln!(file, "# {title}")?;
        self.id_for_path(&path)
    }

    fn create_directory(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        let path = self
            .absolute_parent(parent)?
            .join(Self::validate_name(name)?);
        fs::create_dir(&path)?;
        self.id_for_path(&path)
    }

    fn rename(&self, id: &str, new_name: &str) -> io::Result<EntryId> {
        let source = self.absolute_existing(id)?;
        let mut name = Self::validate_name(new_name)?.to_string();
        if source.is_file()
            && source
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            && !name.to_ascii_lowercase().ends_with(".md")
        {
            name.push_str(".md");
        }
        let destination = source
            .parent()
            .ok_or_else(|| io::Error::other("entry has no parent"))?
            .join(name);
        if destination.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "an entry with that name already exists",
            ));
        }
        fs::rename(&source, &destination)?;
        self.id_for_path(&destination)
    }

    fn move_entry(&self, id: &str, destination_parent: &str) -> io::Result<EntryId> {
        let source = self.absolute_existing(id)?;
        let destination_directory = self.absolute_parent(destination_parent)?;
        if source.is_dir() && destination_directory.starts_with(&source) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a folder cannot be moved into itself",
            ));
        }
        let name = source
            .file_name()
            .ok_or_else(|| io::Error::other("entry has no name"))?;
        let destination = destination_directory.join(name);
        if destination == source {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "entry is already in that folder",
            ));
        }
        if destination.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "an entry with that name already exists in the destination folder",
            ));
        }
        fs::rename(&source, &destination)?;
        self.id_for_path(&destination)
    }

    fn delete(&self, id: &str) -> io::Result<()> {
        let path = self.absolute_existing(id)?;
        let trash = self.root.join(".markerup-trash");
        fs::create_dir_all(&trash)?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("entry has no name"))?
            .to_string_lossy();
        let destination = trash.join(format!("{}-{name}", recovery_suffix()?));
        fs::rename(path, destination)
    }

    fn search_markdown(&self, query: &str) -> io::Result<Vec<EntryId>> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut results = Vec::new();
        for id in self.markdown_files()? {
            let path_match = id.to_lowercase().contains(&query);
            let content_match = self
                .read(&id)
                .map(|c| c.to_lowercase().contains(&query))
                .unwrap_or(false);
            if path_match || content_match {
                results.push(id);
            }
        }
        Ok(results)
    }

    fn resolve_markdown_link(&self, current_file: &str, link: &str) -> Option<LinkTarget> {
        let (path_part, fragment) = link.split_once('#').unwrap_or((link, ""));
        let path_part = path_part.split('?').next()?.trim();
        let canonical = self.resolved_relative_link(current_file, path_part)?;
        if !canonical.is_file()
            || !canonical
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            return None;
        }
        Some(LinkTarget {
            id: self.id_for_path(&canonical).ok()?,
            anchor: (!fragment.is_empty()).then(|| {
                percent_decode_str(fragment)
                    .decode_utf8_lossy()
                    .into_owned()
            }),
        })
    }

    fn resolve_asset_link(&self, current_file: &str, link: &str) -> Option<EntryId> {
        let path_part = link.split(['#', '?']).next()?.trim();
        let canonical = self.resolved_relative_link(current_file, path_part)?;
        canonical
            .is_file()
            .then(|| self.id_for_path(&canonical).ok())
            .flatten()
    }

    fn display_name(&self) -> String {
        self.root_display()
    }
    fn identity(&self) -> String {
        self.root_display()
    }
    fn asset_path(&self, id: &str) -> io::Result<Option<PathBuf>> {
        self.absolute_asset_path(id).map(Some)
    }
    fn asset_bytes(&self, id: &str) -> io::Result<Vec<u8>> {
        fs::read(self.absolute_asset_path(id)?)
    }
}

#[derive(Clone, Default)]
#[allow(dead_code)]
pub enum WorkspaceSlot {
    #[default]
    Empty,
    Local(LocalWorkspace),
    Smb(Arc<SmbWorkspace>),
    #[cfg(target_os = "ios")]
    Ios(Arc<IosWorkspace>),
}

impl WorkspaceSlot {
    #[allow(dead_code)]
    pub fn local(workspace: LocalWorkspace) -> Self {
        Self::Local(workspace)
    }
    pub fn smb(workspace: SmbWorkspace) -> Self {
        Self::Smb(Arc::new(workspace))
    }
    #[cfg(target_os = "ios")]
    pub fn ios(workspace: IosWorkspace) -> Self {
        Self::Ios(Arc::new(workspace))
    }
    pub fn is_open(&self) -> bool {
        match self {
            Self::Local(_) => true,
            Self::Smb(_) => true,
            #[cfg(target_os = "ios")]
            Self::Ios(_) => true,
            Self::Empty => false,
        }
    }
    pub fn root_path(&self) -> Option<&Path> {
        match self {
            Self::Local(workspace) => Some(workspace.root_path()),
            Self::Smb(_) => None,
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => Some(workspace.local.root_path()),
            Self::Empty => None,
        }
    }
    pub fn root_display(&self) -> String {
        match self {
            Self::Local(workspace) => workspace.root_display(),
            Self::Smb(workspace) => workspace.display_name(),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.display_name(),
            Self::Empty => String::new(),
        }
    }
    #[allow(dead_code)]
    pub fn shared_clone(&self) -> Option<WorkspaceRef> {
        match self {
            Self::Local(workspace) => Some(Arc::new(workspace.clone())),
            Self::Smb(workspace) => Some(workspace.clone()),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => Some(workspace.clone()),
            Self::Empty => None,
        }
    }

    pub fn smb_config(&self) -> Option<crate::smb_workspace::SmbConnectionConfig> {
        match self {
            Self::Smb(workspace) => Some(workspace.connection_config()),
            _ => None,
        }
    }
    pub fn absolute_asset_path(&self, id: &str) -> io::Result<PathBuf> {
        match self {
            Self::Local(workspace) => workspace.absolute_asset_path(id),
            Self::Smb(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "SMB assets are not exposed as local paths",
            )),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.local.absolute_asset_path(id),
            Self::Empty => Err(no_workspace()),
        }
    }

    #[cfg(target_os = "ios")]
    #[allow(dead_code)]
    pub fn bookmark(&self) -> Option<Vec<u8>> {
        match self {
            Self::Ios(workspace) => Some(workspace.selection().bookmark),
            _ => None,
        }
    }

    #[cfg(not(target_os = "ios"))]
    #[allow(dead_code)]
    pub fn bookmark(&self) -> Option<Vec<u8>> {
        None
    }
}

fn no_workspace() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "no workspace selected")
}

fn is_supported_asset(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg"
            )
        })
}

impl Workspace for WorkspaceSlot {
    fn entries(&self) -> io::Result<Vec<WorkspaceEntry>> {
        match self {
            Self::Local(workspace) => workspace.entries(),
            Self::Smb(workspace) => workspace.entries(),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.entries(),
            Self::Empty => Err(no_workspace()),
        }
    }
    fn asset_entries(&self) -> io::Result<Vec<WorkspaceEntry>> {
        match self {
            Self::Local(workspace) => workspace.asset_entries(),
            Self::Smb(workspace) => workspace.asset_entries(),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.asset_entries(),
            Self::Empty => Err(no_workspace()),
        }
    }
    fn entries_with_cancel(
        &self,
        should_cancel: &dyn Fn() -> bool,
    ) -> io::Result<Option<Vec<WorkspaceEntry>>> {
        match self {
            Self::Local(workspace) => workspace.entries_with_cancel(should_cancel),
            Self::Smb(workspace) => workspace.entries_with_cancel(should_cancel),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.entries_with_cancel(should_cancel),
            Self::Empty => Err(no_workspace()),
        }
    }
    fn markdown_files(&self) -> io::Result<Vec<EntryId>> {
        match self {
            Self::Local(workspace) => workspace.markdown_files(),
            Self::Smb(workspace) => workspace.markdown_files(),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.markdown_files(),
            Self::Empty => Err(no_workspace()),
        }
    }
    fn read(&self, id: &str) -> io::Result<String> {
        match self {
            Self::Local(workspace) => workspace.read(id),
            Self::Smb(workspace) => workspace.read(id),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.read(id),
            Self::Empty => Err(no_workspace()),
        }
    }
    fn write(&self, id: &str, contents: &str) -> io::Result<()> {
        match self {
            Self::Local(workspace) => workspace.write(id, contents),
            Self::Smb(workspace) => workspace.write(id, contents),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.write(id, contents),
            Self::Empty => Err(no_workspace()),
        }
    }
    fn create_note(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        match self {
            Self::Local(workspace) => workspace.create_note(parent, name),
            Self::Smb(workspace) => workspace.create_note(parent, name),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.create_note(parent, name),
            Self::Empty => Err(no_workspace()),
        }
    }
    fn create_directory(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        match self {
            Self::Local(workspace) => workspace.create_directory(parent, name),
            Self::Smb(workspace) => workspace.create_directory(parent, name),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.create_directory(parent, name),
            Self::Empty => Err(no_workspace()),
        }
    }
    fn rename(&self, id: &str, new_name: &str) -> io::Result<EntryId> {
        match self {
            Self::Local(workspace) => workspace.rename(id, new_name),
            Self::Smb(workspace) => workspace.rename(id, new_name),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.rename(id, new_name),
            Self::Empty => Err(no_workspace()),
        }
    }
    fn move_entry(&self, id: &str, destination_parent: &str) -> io::Result<EntryId> {
        match self {
            Self::Local(workspace) => workspace.move_entry(id, destination_parent),
            Self::Smb(workspace) => workspace.move_entry(id, destination_parent),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.move_entry(id, destination_parent),
            Self::Empty => Err(no_workspace()),
        }
    }
    fn delete(&self, id: &str) -> io::Result<()> {
        match self {
            Self::Local(workspace) => workspace.delete(id),
            Self::Smb(workspace) => workspace.delete(id),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.delete(id),
            Self::Empty => Err(no_workspace()),
        }
    }
    fn search_markdown(&self, query: &str) -> io::Result<Vec<EntryId>> {
        match self {
            Self::Local(workspace) => workspace.search_markdown(query),
            Self::Smb(workspace) => workspace.search_markdown(query),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.search_markdown(query),
            Self::Empty => Err(no_workspace()),
        }
    }
    fn resolve_markdown_link(&self, current_file: &str, link: &str) -> Option<LinkTarget> {
        match self {
            Self::Local(workspace) => workspace.resolve_markdown_link(current_file, link),
            Self::Smb(workspace) => workspace.resolve_markdown_link(current_file, link),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.resolve_markdown_link(current_file, link),
            Self::Empty => None,
        }
    }
    fn resolve_asset_link(&self, current_file: &str, link: &str) -> Option<EntryId> {
        match self {
            Self::Local(workspace) => workspace.resolve_asset_link(current_file, link),
            Self::Smb(workspace) => workspace.resolve_asset_link(current_file, link),
            #[cfg(target_os = "ios")]
            Self::Ios(workspace) => workspace.resolve_asset_link(current_file, link),
            Self::Empty => None,
        }
    }
    fn display_name(&self) -> String {
        self.root_display()
    }
    fn identity(&self) -> String {
        self.root_display()
    }
    fn asset_path(&self, id: &str) -> io::Result<Option<PathBuf>> {
        match self {
            Self::Smb(workspace) => workspace.asset_path(id),
            _ => self.absolute_asset_path(id).map(Some),
        }
    }
    fn asset_bytes(&self, id: &str) -> io::Result<Vec<u8>> {
        match self {
            Self::Smb(workspace) => workspace.asset_bytes(id),
            _ => self
                .asset_path(id)?
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "asset has no path"))
                .and_then(fs::read),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EntryKind, LocalWorkspace, Workspace, backup_directory_id, backup_snapshot_name,
        is_backup_snapshot_name, retained_backup_names,
    };
    use std::collections::HashSet;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("markerup-test-{unique}"));
        fs::create_dir_all(root.join("nested/deeper")).unwrap();
        fs::write(root.join("Root.md"), "# Root").unwrap();
        fs::write(root.join("nested/Other.md"), "# Other").unwrap();
        fs::write(root.join("nested/deeper/Deep.md"), "# Deep").unwrap();
        root
    }

    fn snapshot_name(day: u128, second_of_day: u128, process_id: u32) -> String {
        let timestamp = (day * 86_400 + second_of_day) * 1_000_000_000;
        backup_snapshot_name(&format!("{timestamp:020}-{process_id}"))
    }

    #[test]
    fn backup_directory_mirrors_note_path_under_markerup() {
        assert_eq!(
            backup_directory_id("games/minecraft.md").unwrap(),
            ".markerup/backups/games/minecraft.md"
        );
        assert_eq!(
            backup_directory_id("Notes.md").unwrap(),
            ".markerup/backups/Notes.md"
        );
        assert!(backup_directory_id("../outside.md").is_err());
    }

    #[test]
    fn backup_retention_keeps_recent_daily_and_monthly_snapshots() {
        // 1970-06-11 in UTC; the daily window covers June 7-11 and the
        // monthly window covers February-June.
        let today = 161_u128;
        let now = (today * 86_400 + 86_399) * 1_000_000_000;
        let mut names = Vec::new();
        // The five newest saves are all on the current day.
        for second in 43_200..43_205 {
            names.push(snapshot_name(today, second, second as u32));
        }
        // One save for each other day in the five-day window.
        for day in [157_u128, 158, 159, 160] {
            names.push(snapshot_name(day, 43_200, day as u32));
        }
        // The latest saves in the preceding four months, plus older
        // snapshots in those months that should be pruned.
        for (day, process_id) in [(150_u128, 50), (119, 40), (89, 30), (58, 20)] {
            names.push(snapshot_name(day, 43_200, process_id));
            names.push(snapshot_name(day - 1, 43_200, process_id - 1));
        }
        let outside_window = snapshot_name(30, 43_200, 1);
        names.push(outside_window.clone());
        names.push("README.md".to_string());

        let retained = retained_backup_names(&names, now);
        let expected: HashSet<_> = names
            .iter()
            .filter(|name| {
                is_backup_snapshot_name(name)
                    && *name != &outside_window
                    && ![(149_u128, 49), (118, 39), (88, 29), (57, 19)]
                        .iter()
                        .any(|(day, process_id)| *name == &snapshot_name(*day, 43_200, *process_id))
            })
            .cloned()
            .collect();
        assert_eq!(retained, expected);
        assert!(retained.len() <= 15);
        assert!(!retained.contains(&"README.md".to_string()));
    }

    #[test]
    fn scans_hierarchy() {
        let root = temp();
        let w = LocalWorkspace::open(&root).unwrap();
        let entries = w.entries().unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.id == "nested" && e.kind == EntryKind::Directory && e.depth == 0)
        );
        assert!(
            entries
                .iter()
                .any(|e| e.id == "nested/Other.md" && e.depth == 1)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scans_supported_assets_without_treating_them_as_notes() {
        let root = temp();
        fs::write(root.join("diagram.SVG"), "<svg></svg>").unwrap();
        fs::write(root.join("nested/photo.JPG"), b"image").unwrap();
        fs::write(root.join("nested/readme.txt"), "not an image").unwrap();
        let w = LocalWorkspace::open(&root).unwrap();

        let assets = w.asset_entries().unwrap();
        assert!(assets.iter().any(|entry| entry.id == "diagram.SVG"));
        assert!(assets.iter().any(|entry| entry.id == "nested/photo.JPG"));
        assert!(assets.iter().all(|entry| entry.kind == EntryKind::File));
        assert!(!assets.iter().any(|entry| entry.id.ends_with("readme.txt")));
        assert!(
            !w.entries()
                .unwrap()
                .iter()
                .any(|entry| entry.id == "diagram.SVG")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignores_hidden_directories() {
        let root = temp();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::write(root.join(".git/Hidden.md"), "# hidden").unwrap();
        let w = LocalWorkspace::open(&root).unwrap();
        assert!(
            w.entries()
                .unwrap()
                .iter()
                .all(|entry| !entry.id.starts_with(".git"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mutates_plain_files_and_folders() {
        let root = temp();
        let w = LocalWorkspace::open(&root).unwrap();
        let folder = w.create_directory("", "New").unwrap();
        let note = w.create_note(&folder, "Note").unwrap();
        w.write(&note, "hello").unwrap();
        assert_eq!(w.read(&note).unwrap(), "hello");
        let renamed = w.rename(&note, "Renamed").unwrap();
        assert_eq!(renamed, "New/Renamed.md");
        w.delete(&folder).unwrap();
        assert!(!root.join("New").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacement_is_atomic_and_keeps_the_previous_note_if_write_fails() {
        let root = temp();
        let legacy_backup = root.join(".Root.md.markerup-backup-legacy");
        fs::write(&legacy_backup, "older recovery copy").unwrap();
        let w = LocalWorkspace::open(&root).unwrap();
        w.write("Root.md", "# Updated").unwrap();
        assert_eq!(w.read("Root.md").unwrap(), "# Updated");
        let backup_directory = root.join(".markerup/backups/Root.md");
        let backup = fs::read_dir(&backup_directory)
            .unwrap()
            .filter_map(Result::ok)
            .find(|item| is_backup_snapshot_name(&item.file_name().to_string_lossy()))
            .unwrap()
            .path();
        assert_eq!(fs::read_to_string(backup).unwrap(), "# Root");
        assert_eq!(
            fs::read_to_string(legacy_backup).unwrap(),
            "older recovery copy"
        );
        assert!(!fs::read_dir(&root).unwrap().any(|item| {
            item.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("markerup-writing")
        }));
        // An invalid note must not create a new file or touch another note.
        assert!(w.write("Missing.md", "replacement").is_err());
        assert_eq!(w.read("Root.md").unwrap(), "# Updated");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleted_note_and_nested_folder_remain_recoverable() {
        let root = temp();
        let w = LocalWorkspace::open(&root).unwrap();
        w.delete("Root.md").unwrap();
        w.delete("nested").unwrap();
        let trashed: Vec<_> = fs::read_dir(root.join(".markerup-trash"))
            .unwrap()
            .map(|item| item.unwrap().path())
            .collect();
        assert!(trashed.iter().any(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("-Root.md")
                && fs::read_to_string(path).unwrap() == "# Root"
        }));
        assert!(
            trashed
                .iter()
                .any(|path| path.join("deeper/Deep.md").exists())
        );
        assert!(w.entries().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn moves_notes_and_folders() {
        let root = temp();
        let w = LocalWorkspace::open(&root).unwrap();
        let destination = w.create_directory("", "Destination").unwrap();
        let note = w.move_entry("Root.md", &destination).unwrap();
        assert_eq!(note, "Destination/Root.md");
        let folder = w.move_entry("nested", &destination).unwrap();
        assert_eq!(folder, "Destination/nested");
        assert!(root.join("Destination/nested/deeper/Deep.md").exists());
        assert!(w.move_entry("Destination", "Destination/nested").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_encoded_parent_links_and_anchors() {
        let root = temp();
        fs::write(root.join("With Space.md"), "# Target").unwrap();
        let w = LocalWorkspace::open(&root).unwrap();
        let target = w
            .resolve_markdown_link("nested/Other.md", "../With%20Space.md#target")
            .unwrap();
        assert_eq!(target.id, "With Space.md");
        assert_eq!(target.anchor.as_deref(), Some("target"));
        assert!(
            w.resolve_markdown_link("Root.md", "../outside.md")
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn searches_names_and_contents() {
        let root = temp();
        fs::write(root.join("Root.md"), "special phrase").unwrap();
        let w = LocalWorkspace::open(&root).unwrap();
        assert_eq!(w.search_markdown("special").unwrap(), vec!["Root.md"]);
        assert!(
            w.search_markdown("other")
                .unwrap()
                .contains(&"nested/Other.md".to_string())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellable_scan_can_stop_before_building_entries() {
        let root = temp();
        let w = LocalWorkspace::open(&root).unwrap();
        assert!(w.entries_with_cancel(|| true).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
