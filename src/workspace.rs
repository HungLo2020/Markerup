use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub trait Workspace {
    fn root(&self) -> &Path;
    fn markdown_files(&self) -> io::Result<Vec<PathBuf>>;
    fn read(&self, relative_path: &Path) -> io::Result<String>;
    fn write(&self, relative_path: &Path, contents: &str) -> io::Result<()>;
    fn resolve_markdown_link(&self, current_file: &Path, link: &str) -> Option<PathBuf>;
}

#[derive(Debug, Clone)]
pub struct LocalWorkspace {
    root: PathBuf,
}

impl LocalWorkspace {
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

    fn absolute_path(&self, relative_path: &Path) -> io::Result<PathBuf> {
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "path escapes the workspace",
            ));
        }
        Ok(self.root.join(relative_path))
    }

    fn scan_dir(&self, directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
        let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                self.scan_dir(&path, files)?;
            } else if file_type.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
            {
                if let Ok(relative) = path.strip_prefix(&self.root) {
                    files.push(relative.to_path_buf());
                }
            }
        }
        Ok(())
    }
}

impl Workspace for LocalWorkspace {
    fn root(&self) -> &Path {
        &self.root
    }

    fn markdown_files(&self) -> io::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        self.scan_dir(&self.root, &mut files)?;
        files.sort();
        Ok(files)
    }

    fn read(&self, relative_path: &Path) -> io::Result<String> {
        fs::read_to_string(self.absolute_path(relative_path)?)
    }

    fn write(&self, relative_path: &Path, contents: &str) -> io::Result<()> {
        fs::write(self.absolute_path(relative_path)?, contents)
    }

    fn resolve_markdown_link(&self, current_file: &Path, link: &str) -> Option<PathBuf> {
        let target = link.split(['#', '?']).next()?.trim();
        if target.is_empty()
            || target.starts_with('/')
            || target.contains("://")
            || target.starts_with("mailto:")
        {
            return None;
        }

        let parent = current_file.parent().unwrap_or_else(|| Path::new(""));
        let candidate = self.root.join(parent).join(target);
        let canonical = fs::canonicalize(candidate).ok()?;
        if !canonical.starts_with(&self.root)
            || !canonical.is_file()
            || !canonical
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            return None;
        }

        canonical
            .strip_prefix(&self.root)
            .ok()
            .map(Path::to_path_buf)
    }
}

#[cfg(test)]
mod tests {
    use super::{LocalWorkspace, Workspace};
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_workspace() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("markerup-test-{unique}"));
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("Root.md"), "# Root").unwrap();
        fs::write(root.join("nested/Other.md"), "# Other").unwrap();
        root
    }

    #[test]
    fn scans_markdown_recursively() {
        let root = temporary_workspace();
        fs::write(root.join("ignore.txt"), "not markdown").unwrap();
        let workspace = LocalWorkspace::open(&root).unwrap();

        assert_eq!(
            workspace.markdown_files().unwrap(),
            vec![
                std::path::PathBuf::from("Root.md"),
                std::path::PathBuf::from("nested/Other.md")
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_relative_markdown_links() {
        let root = temporary_workspace();
        let workspace = LocalWorkspace::open(&root).unwrap();

        assert_eq!(
            workspace.resolve_markdown_link(Path::new("Root.md"), "nested/Other.md#section"),
            Some(std::path::PathBuf::from("nested/Other.md"))
        );
        assert_eq!(
            workspace.resolve_markdown_link(Path::new("Root.md"), "https://example.com"),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }
}
