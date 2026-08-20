use percent_encoding::percent_decode_str;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

pub type EntryId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind { File, Directory }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub id: EntryId,
    pub name: String,
    pub kind: EntryKind,
    pub depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTarget {
    pub id: EntryId,
    pub anchor: Option<String>,
}

pub trait Workspace {
    fn entries(&self) -> io::Result<Vec<WorkspaceEntry>>;
    fn markdown_files(&self) -> io::Result<Vec<EntryId>>;
    fn read(&self, id: &str) -> io::Result<String>;
    fn write(&self, id: &str, contents: &str) -> io::Result<()>;
    fn create_note(&self, parent: &str, name: &str) -> io::Result<EntryId>;
    fn create_directory(&self, parent: &str, name: &str) -> io::Result<EntryId>;
    fn rename(&self, id: &str, new_name: &str) -> io::Result<EntryId>;
    fn delete(&self, id: &str) -> io::Result<()>;
    fn search_markdown(&self, query: &str) -> io::Result<Vec<EntryId>>;
    fn resolve_markdown_link(&self, current_file: &str, link: &str) -> Option<LinkTarget>;
    fn resolve_asset_link(&self, current_file: &str, link: &str) -> Option<EntryId>;
}

#[derive(Debug, Clone)]
pub struct LocalWorkspace { root: PathBuf }

impl LocalWorkspace {
    pub fn open(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "workspace path is not a directory"));
        }
        Ok(Self { root })
    }

    pub fn root_path(&self) -> &Path { &self.root }
    pub fn root_display(&self) -> String { self.root.to_string_lossy().into_owned() }
    pub fn absolute_asset_path(&self, id: &str) -> io::Result<PathBuf> { self.absolute_existing(id) }

    fn validate_id(id: &str) -> io::Result<PathBuf> {
        let path = Path::new(id);
        if path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "entry escapes the workspace"));
        }
        Ok(path.to_path_buf())
    }

    fn validate_name(name: &str) -> io::Result<&str> {
        let name = name.trim();
        if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "name must be a single non-empty path component"));
        }
        Ok(name)
    }

    fn absolute_existing(&self, id: &str) -> io::Result<PathBuf> {
        let relative = Self::validate_id(id)?;
        let canonical = fs::canonicalize(self.root.join(relative))?;
        if !canonical.starts_with(&self.root) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "entry escapes the workspace"));
        }
        Ok(canonical)
    }

    fn absolute_parent(&self, parent: &str) -> io::Result<PathBuf> {
        if parent.is_empty() { return Ok(self.root.clone()); }
        let path = self.absolute_existing(parent)?;
        if !path.is_dir() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "parent is not a directory"));
        }
        Ok(path)
    }

    fn id_for_path(&self, path: &Path) -> io::Result<EntryId> {
        let relative = path.strip_prefix(&self.root).map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "entry escapes workspace"))?;
        Ok(relative.to_string_lossy().replace('\\', "/"))
    }

    fn scan_dir(&self, directory: &Path, depth: usize, entries: &mut Vec<WorkspaceEntry>) -> io::Result<()> {
        let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
        for child in children {
            let name = child.file_name();
            let name_text = name.to_string_lossy();
            if name_text.starts_with('.') { continue; }

            let path = child.path();
            let ty = child.file_type()?;
            if ty.is_dir() {
                entries.push(WorkspaceEntry {
                    id: self.id_for_path(&path)?,
                    name: name_text.into_owned(),
                    kind: EntryKind::Directory,
                    depth,
                });
                self.scan_dir(&path, depth + 1, entries)?;
            } else if ty.is_file() && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md")) {
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

    fn resolved_relative_link(&self, current_file: &str, raw: &str) -> Option<PathBuf> {
        let decoded = percent_decode_str(raw).decode_utf8().ok()?;
        let target = decoded.trim();
        if target.is_empty() || target.starts_with('/') || target.contains("://") || target.starts_with("mailto:") { return None; }
        let current = Self::validate_id(current_file).ok()?;
        let parent = current.parent().unwrap_or_else(|| Path::new(""));
        let canonical = fs::canonicalize(self.root.join(parent).join(target)).ok()?;
        canonical.starts_with(&self.root).then_some(canonical)
    }
}

impl Workspace for LocalWorkspace {
    fn entries(&self) -> io::Result<Vec<WorkspaceEntry>> {
        let mut entries = Vec::new();
        self.scan_dir(&self.root, 0, &mut entries)?;
        Ok(entries)
    }

    fn markdown_files(&self) -> io::Result<Vec<EntryId>> {
        Ok(self.entries()?.into_iter().filter(|e| e.kind == EntryKind::File).map(|e| e.id).collect())
    }

    fn read(&self, id: &str) -> io::Result<String> { fs::read_to_string(self.absolute_existing(id)?) }

    fn write(&self, id: &str, contents: &str) -> io::Result<()> {
        let path = self.absolute_existing(id)?;
        if !path.is_file() { return Err(io::Error::new(io::ErrorKind::InvalidInput, "entry is not a file")); }
        fs::write(path, contents)
    }

    fn create_note(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        let mut name = Self::validate_name(name)?.to_string();
        if !name.to_ascii_lowercase().ends_with(".md") { name.push_str(".md"); }
        let path = self.absolute_parent(parent)?.join(name);
        let mut file = OpenOptions::new().write(true).create_new(true).open(&path)?;
        let title = path.file_stem().unwrap_or_default().to_string_lossy();
        writeln!(file, "# {title}")?;
        self.id_for_path(&path)
    }

    fn create_directory(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        let path = self.absolute_parent(parent)?.join(Self::validate_name(name)?);
        fs::create_dir(&path)?;
        self.id_for_path(&path)
    }

    fn rename(&self, id: &str, new_name: &str) -> io::Result<EntryId> {
        let source = self.absolute_existing(id)?;
        let mut name = Self::validate_name(new_name)?.to_string();
        if source.is_file() && source.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md")) && !name.to_ascii_lowercase().ends_with(".md") { name.push_str(".md"); }
        let destination = source.parent().ok_or_else(|| io::Error::other("entry has no parent"))?.join(name);
        if destination.exists() { return Err(io::Error::new(io::ErrorKind::AlreadyExists, "an entry with that name already exists")); }
        fs::rename(&source, &destination)?;
        self.id_for_path(&destination)
    }

    fn delete(&self, id: &str) -> io::Result<()> {
        let path = self.absolute_existing(id)?;
        if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
    }

    fn search_markdown(&self, query: &str) -> io::Result<Vec<EntryId>> {
        let query = query.trim().to_lowercase();
        if query.is_empty() { return Ok(Vec::new()); }
        let mut results = Vec::new();
        for id in self.markdown_files()? {
            let path_match = id.to_lowercase().contains(&query);
            let content_match = self.read(&id).map(|c| c.to_lowercase().contains(&query)).unwrap_or(false);
            if path_match || content_match { results.push(id); }
        }
        Ok(results)
    }

    fn resolve_markdown_link(&self, current_file: &str, link: &str) -> Option<LinkTarget> {
        let (path_part, fragment) = link.split_once('#').unwrap_or((link, ""));
        let path_part = path_part.split('?').next()?.trim();
        let canonical = self.resolved_relative_link(current_file, path_part)?;
        if !canonical.is_file() || !canonical.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md")) { return None; }
        Some(LinkTarget {
            id: self.id_for_path(&canonical).ok()?,
            anchor: (!fragment.is_empty()).then(|| percent_decode_str(fragment).decode_utf8_lossy().into_owned()),
        })
    }

    fn resolve_asset_link(&self, current_file: &str, link: &str) -> Option<EntryId> {
        let path_part = link.split(['#', '?']).next()?.trim();
        let canonical = self.resolved_relative_link(current_file, path_part)?;
        canonical.is_file().then(|| self.id_for_path(&canonical).ok()).flatten()
    }
}

#[derive(Debug, Clone, Default)]
pub enum WorkspaceSlot {
    #[default]
    Empty,
    Local(LocalWorkspace),
}

impl WorkspaceSlot {
    pub fn local(workspace: LocalWorkspace) -> Self { Self::Local(workspace) }
    pub fn is_open(&self) -> bool { matches!(self, Self::Local(_)) }
    pub fn root_path(&self) -> Option<&Path> {
        match self { Self::Local(workspace) => Some(workspace.root_path()), Self::Empty => None }
    }
    pub fn root_display(&self) -> String {
        match self { Self::Local(workspace) => workspace.root_display(), Self::Empty => String::new() }
    }
    pub fn absolute_asset_path(&self, id: &str) -> io::Result<PathBuf> {
        match self {
            Self::Local(workspace) => workspace.absolute_asset_path(id),
            Self::Empty => Err(no_workspace()),
        }
    }
}

fn no_workspace() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "no workspace selected")
}

impl Workspace for WorkspaceSlot {
    fn entries(&self) -> io::Result<Vec<WorkspaceEntry>> {
        match self { Self::Local(workspace) => workspace.entries(), Self::Empty => Ok(Vec::new()) }
    }
    fn markdown_files(&self) -> io::Result<Vec<EntryId>> {
        match self { Self::Local(workspace) => workspace.markdown_files(), Self::Empty => Ok(Vec::new()) }
    }
    fn read(&self, id: &str) -> io::Result<String> {
        match self { Self::Local(workspace) => workspace.read(id), Self::Empty => Err(no_workspace()) }
    }
    fn write(&self, id: &str, contents: &str) -> io::Result<()> {
        match self { Self::Local(workspace) => workspace.write(id, contents), Self::Empty => Err(no_workspace()) }
    }
    fn create_note(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        match self { Self::Local(workspace) => workspace.create_note(parent, name), Self::Empty => Err(no_workspace()) }
    }
    fn create_directory(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        match self { Self::Local(workspace) => workspace.create_directory(parent, name), Self::Empty => Err(no_workspace()) }
    }
    fn rename(&self, id: &str, new_name: &str) -> io::Result<EntryId> {
        match self { Self::Local(workspace) => workspace.rename(id, new_name), Self::Empty => Err(no_workspace()) }
    }
    fn delete(&self, id: &str) -> io::Result<()> {
        match self { Self::Local(workspace) => workspace.delete(id), Self::Empty => Err(no_workspace()) }
    }
    fn search_markdown(&self, query: &str) -> io::Result<Vec<EntryId>> {
        match self { Self::Local(workspace) => workspace.search_markdown(query), Self::Empty => Ok(Vec::new()) }
    }
    fn resolve_markdown_link(&self, current_file: &str, link: &str) -> Option<LinkTarget> {
        match self { Self::Local(workspace) => workspace.resolve_markdown_link(current_file, link), Self::Empty => None }
    }
    fn resolve_asset_link(&self, current_file: &str, link: &str) -> Option<EntryId> {
        match self { Self::Local(workspace) => workspace.resolve_asset_link(current_file, link), Self::Empty => None }
    }
}

#[cfg(test)]
mod tests {
    use super::{EntryKind, LocalWorkspace, Workspace};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp() -> std::path::PathBuf {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("markerup-test-{unique}"));
        fs::create_dir_all(root.join("nested/deeper")).unwrap();
        fs::write(root.join("Root.md"), "# Root").unwrap();
        fs::write(root.join("nested/Other.md"), "# Other").unwrap();
        fs::write(root.join("nested/deeper/Deep.md"), "# Deep").unwrap();
        root
    }

    #[test]
    fn scans_hierarchy() {
        let root = temp();
        let w = LocalWorkspace::open(&root).unwrap();
        let entries = w.entries().unwrap();
        assert!(entries.iter().any(|e| e.id == "nested" && e.kind == EntryKind::Directory && e.depth == 0));
        assert!(entries.iter().any(|e| e.id == "nested/Other.md" && e.depth == 1));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignores_hidden_directories() {
        let root = temp();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::write(root.join(".git/Hidden.md"), "# hidden").unwrap();
        let w = LocalWorkspace::open(&root).unwrap();
        assert!(w.entries().unwrap().iter().all(|entry| !entry.id.starts_with(".git")));
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
    fn resolves_encoded_parent_links_and_anchors() {
        let root = temp();
        fs::write(root.join("With Space.md"), "# Target").unwrap();
        let w = LocalWorkspace::open(&root).unwrap();
        let target = w.resolve_markdown_link("nested/Other.md", "../With%20Space.md#target").unwrap();
        assert_eq!(target.id, "With Space.md");
        assert_eq!(target.anchor.as_deref(), Some("target"));
        assert!(w.resolve_markdown_link("Root.md", "../outside.md").is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn searches_names_and_contents() {
        let root = temp();
        fs::write(root.join("Root.md"), "special phrase").unwrap();
        let w = LocalWorkspace::open(&root).unwrap();
        assert_eq!(w.search_markdown("special").unwrap(), vec!["Root.md"]);
        assert!(w.search_markdown("other").unwrap().contains(&"nested/Other.md".to_string()));
        fs::remove_dir_all(root).unwrap();
    }
}
