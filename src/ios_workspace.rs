#![cfg(target_os = "ios")]

use crate::ios_bridge::{
    WorkspaceSelection, ensure_directory, list_entries, mutate, read_file, stop_access, write_file,
};
use crate::workspace::{
    EntryId, LinkTarget, LocalWorkspace, Workspace, WorkspaceEntry, backup_directory_id,
    backup_snapshot_name, is_backup_snapshot_name, recovery_suffix, retained_backup_names,
    unix_time_nanos,
};
use std::io;
use std::path::{Path, PathBuf};

pub struct IosWorkspace {
    pub(crate) local: LocalWorkspace,
    bookmark: Vec<u8>,
}

impl IosWorkspace {
    pub fn open(selection: WorkspaceSelection) -> io::Result<Self> {
        Ok(Self {
            local: LocalWorkspace::open_scoped(selection.path)?,
            bookmark: selection.bookmark,
        })
    }

    pub fn selection(&self) -> WorkspaceSelection {
        WorkspaceSelection {
            path: self.local.root_path().to_path_buf(),
            bookmark: self.bookmark.clone(),
        }
    }

    fn scoped_path(&self, id: &str) -> io::Result<PathBuf> {
        self.local.scoped_path(id)
    }

    fn prune_backups(&self, directory: &Path) {
        let Ok(bytes) = list_entries(directory) else {
            return;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            return;
        };
        let names: Vec<String> = text
            .lines()
            .filter_map(|line| line.strip_prefix("F:"))
            .filter_map(|id| {
                percent_encoding::percent_decode_str(id)
                    .decode_utf8()
                    .ok()
                    .map(|name| name.into_owned())
            })
            .collect();
        let Ok(now_nanos) = unix_time_nanos() else {
            return;
        };
        let retained = retained_backup_names(&names, now_nanos);
        for name in names {
            if is_backup_snapshot_name(&name) && !retained.contains(&name) {
                let _ = mutate(&directory.join(name), None, 3, &[]);
            }
        }
    }

    fn coordinated_entries(&self) -> io::Result<Vec<WorkspaceEntry>> {
        let bytes = list_entries(self.local.root_path()).map_err(io::Error::other)?;
        let text = String::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut entries = Vec::new();
        for line in text.lines() {
            let Some((kind, encoded_id)) = line.split_once(':') else {
                continue;
            };
            let id = percent_encoding::percent_decode_str(encoded_id)
                .decode_utf8()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                .into_owned();
            let path = Path::new(&id);
            let Some(name) = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            let depth = path.components().count().saturating_sub(1);
            if kind == "D" {
                entries.push(WorkspaceEntry {
                    id,
                    name,
                    kind: crate::workspace::EntryKind::Directory,
                    depth,
                });
            } else if kind == "F" {
                entries.push(WorkspaceEntry {
                    id,
                    name,
                    kind: crate::workspace::EntryKind::File,
                    depth,
                });
            }
        }
        entries.sort_by_key(|entry| entry.id.to_lowercase());
        Ok(entries)
    }

    fn coordinated_assets(&self) -> io::Result<Vec<WorkspaceEntry>> {
        let bytes = list_entries(self.local.root_path()).map_err(io::Error::other)?;
        let text = String::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut entries = Vec::new();
        for line in text.lines() {
            let Some((kind, encoded_id)) = line.split_once(':') else {
                continue;
            };
            if kind != "A" {
                continue;
            }
            let id = percent_encoding::percent_decode_str(encoded_id)
                .decode_utf8()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                .into_owned();
            let path = Path::new(&id);
            let Some(name) = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            let depth = path.components().count().saturating_sub(1);
            entries.push(WorkspaceEntry {
                id,
                name,
                kind: crate::workspace::EntryKind::File,
                depth,
            });
        }
        entries.sort_by_key(|entry| entry.id.to_lowercase());
        Ok(entries)
    }
}

impl Drop for IosWorkspace {
    fn drop(&mut self) {
        stop_access(self.local.root_path());
    }
}

impl Workspace for IosWorkspace {
    fn entries(&self) -> io::Result<Vec<WorkspaceEntry>> {
        self.coordinated_entries()
    }
    fn asset_entries(&self) -> io::Result<Vec<WorkspaceEntry>> {
        self.coordinated_assets()
    }
    fn entries_with_cancel(
        &self,
        cancel: &dyn Fn() -> bool,
    ) -> io::Result<Option<Vec<WorkspaceEntry>>> {
        if cancel() {
            return Ok(None);
        }
        let entries = self.coordinated_entries()?;
        Ok((!cancel()).then_some(entries))
    }
    fn markdown_files(&self) -> io::Result<Vec<EntryId>> {
        Ok(self
            .entries()?
            .into_iter()
            .filter(|entry| entry.kind == crate::workspace::EntryKind::File)
            .map(|entry| entry.id)
            .collect())
    }
    fn read(&self, id: &str) -> io::Result<String> {
        let path = self.scoped_path(id)?;
        String::from_utf8(read_file(&path).map_err(io::Error::other)?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
    fn write(&self, id: &str, contents: &str) -> io::Result<()> {
        let path = self.scoped_path(id)?;
        let previous = read_file(&path).map_err(io::Error::other)?;
        let backup_directory_id = backup_directory_id(id)?;
        let backup_directory = self.scoped_path(&backup_directory_id)?;
        ensure_directory(&backup_directory).map_err(io::Error::other)?;
        let backup = backup_directory.join(backup_snapshot_name(&recovery_suffix()?));
        // Coordinated backup first: a provider rejecting the backup must not
        // replace the only existing copy of a note.
        mutate(&backup, None, 1, &previous).map_err(io::Error::other)?;
        write_file(&path, contents.as_bytes()).map_err(io::Error::other)?;
        self.prune_backups(&backup_directory);
        Ok(())
    }
    fn create_note(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        let mut name = LocalWorkspace::validate_name(name)?.to_string();
        if !name.to_ascii_lowercase().ends_with(".md") {
            name.push_str(".md");
        }
        let path = self.scoped_path(parent)?.join(name);
        let title = path.file_stem().unwrap_or_default().to_string_lossy();
        let contents = format!("# {title}\n");
        mutate(&path, None, 1, contents.as_bytes()).map_err(io::Error::other)?;
        self.local.id_for_path(&path)
    }
    fn create_directory(&self, parent: &str, name: &str) -> io::Result<EntryId> {
        let path = self
            .scoped_path(parent)?
            .join(LocalWorkspace::validate_name(name)?);
        mutate(&path, None, 0, &[]).map_err(io::Error::other)?;
        self.local.id_for_path(&path)
    }
    fn rename(&self, id: &str, new_name: &str) -> io::Result<EntryId> {
        let source = self.scoped_path(id)?;
        let mut name = LocalWorkspace::validate_name(new_name)?.to_string();
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
        mutate(&source, Some(&destination), 2, &[]).map_err(io::Error::other)?;
        self.local.id_for_path(&destination)
    }
    fn move_entry(&self, id: &str, destination_parent: &str) -> io::Result<EntryId> {
        let source = self.scoped_path(id)?;
        let destination_directory = self.scoped_path(destination_parent)?;
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
        mutate(&source, Some(&destination), 2, &[]).map_err(io::Error::other)?;
        self.local.id_for_path(&destination)
    }
    fn delete(&self, id: &str) -> io::Result<()> {
        let path = self.scoped_path(id)?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("entry has no name"))?
            .to_string_lossy();
        let destination =
            self.scoped_path(&format!(".markerup-trash/{}-{name}", recovery_suffix()?))?;
        mutate(&path, Some(&destination), 4, &[]).map_err(io::Error::other)
    }
    fn search_markdown(&self, query: &str) -> io::Result<Vec<EntryId>> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut results = Vec::new();
        for entry in self
            .entries()?
            .into_iter()
            .filter(|entry| entry.kind == crate::workspace::EntryKind::File)
        {
            let path_match = entry.id.to_lowercase().contains(&query);
            let content_match = !path_match
                && self
                    .read(&entry.id)
                    .map(|text| text.to_lowercase().contains(&query))
                    .unwrap_or(false);
            if path_match || content_match {
                results.push(entry.id);
            }
        }
        Ok(results)
    }
    fn resolve_markdown_link(&self, current_file: &str, link: &str) -> Option<LinkTarget> {
        self.local.resolve_markdown_link(current_file, link)
    }
    fn resolve_asset_link(&self, current_file: &str, link: &str) -> Option<EntryId> {
        self.local.resolve_asset_link(current_file, link)
    }
    fn display_name(&self) -> String {
        self.local.root_display()
    }
    fn identity(&self) -> String {
        format!("ios:{}", self.local.root_display())
    }
    fn asset_path(&self, id: &str) -> io::Result<Option<PathBuf>> {
        self.scoped_path(id).map(Some)
    }
    fn asset_bytes(&self, id: &str) -> io::Result<Vec<u8>> {
        read_file(&self.scoped_path(id)?).map_err(io::Error::other)
    }
}
