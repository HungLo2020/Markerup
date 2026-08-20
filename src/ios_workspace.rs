#![cfg(target_os = "ios")]

use crate::ios_bridge::{mutate, read_file, stop_access, write_file, WorkspaceSelection};
use crate::workspace::{EntryId, LinkTarget, LocalWorkspace, Workspace, WorkspaceEntry};
use std::io;
use std::path::PathBuf;

pub struct IosWorkspace {
    pub(crate) local: LocalWorkspace,
    bookmark: Vec<u8>,
}

impl IosWorkspace {
    pub fn open(selection: WorkspaceSelection) -> io::Result<Self> {
        Ok(Self { local: LocalWorkspace::open(selection.path)?, bookmark: selection.bookmark })
    }

    pub fn selection(&self) -> WorkspaceSelection {
        WorkspaceSelection { path: self.local.root_path().to_path_buf(), bookmark: self.bookmark.clone() }
    }
}

impl Drop for IosWorkspace {
    fn drop(&mut self) { stop_access(self.local.root_path()); }
}

impl Workspace for IosWorkspace {
    fn entries(&self) -> io::Result<Vec<WorkspaceEntry>> { self.local.entries() }
    fn entries_with_cancel(&self, cancel: &dyn Fn() -> bool) -> io::Result<Option<Vec<WorkspaceEntry>>> { self.local.entries_with_cancel(cancel) }
    fn markdown_files(&self) -> io::Result<Vec<EntryId>> { self.local.markdown_files() }
    fn read(&self, id: &str) -> io::Result<String> {
        let path = self.local.absolute_existing(id)?;
        String::from_utf8(read_file(&path).map_err(io::Error::other)?).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
    fn write(&self, id: &str, contents: &str) -> io::Result<()> {
        let path = self.local.absolute_existing(id)?;
        write_file(&path, contents.as_bytes()).map_err(io::Error::other)
    }
    fn create_note(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        let mut name = LocalWorkspace::validate_name(name)?.to_string();
        if !name.to_ascii_lowercase().ends_with(".md") { name.push_str(".md"); }
        let path = self.local.absolute_parent(parent)?.join(name);
        let title = path.file_stem().unwrap_or_default().to_string_lossy();
        let contents = format!("# {title}\n");
        mutate(&path, None, 1, contents.as_bytes()).map_err(io::Error::other)?;
        self.local.id_for_path(&path)
    }
    fn create_directory(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        let path = self.local.absolute_parent(parent)?.join(LocalWorkspace::validate_name(name)?);
        mutate(&path, None, 0, &[]).map_err(io::Error::other)?;
        self.local.id_for_path(&path)
    }
    fn rename(&self, id: &str, new_name: &str) -> io::Result<EntryId> {
        let source = self.local.absolute_existing(id)?;
        let mut name = LocalWorkspace::validate_name(new_name)?.to_string();
        if source.is_file() && source.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md")) && !name.to_ascii_lowercase().ends_with(".md") { name.push_str(".md"); }
        let destination = source.parent().ok_or_else(|| io::Error::other("entry has no parent"))?.join(name);
        if destination.exists() { return Err(io::Error::new(io::ErrorKind::AlreadyExists, "an entry with that name already exists")); }
        mutate(&source, Some(&destination), 2, &[]).map_err(io::Error::other)?;
        self.local.id_for_path(&destination)
    }
    fn delete(&self, id: &str) -> io::Result<()> {
        let path = self.local.absolute_existing(id)?;
        mutate(&path, None, 3, &[]).map_err(io::Error::other)
    }
    fn search_markdown(&self, query: &str) -> io::Result<Vec<EntryId>> { self.local.search_markdown(query) }
    fn resolve_markdown_link(&self, current_file: &str, link: &str) -> Option<LinkTarget> { self.local.resolve_markdown_link(current_file, link) }
    fn resolve_asset_link(&self, current_file: &str, link: &str) -> Option<EntryId> { self.local.resolve_asset_link(current_file, link) }
    fn display_name(&self) -> String { self.local.root_display() }
    fn identity(&self) -> String { format!("ios:{}", self.local.root_display()) }
    fn asset_path(&self, id: &str) -> io::Result<Option<PathBuf>> { self.local.absolute_asset_path(id).map(Some) }
}
