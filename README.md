# Markerup

Markerup is a filesystem-first Markdown notes editor. The filesystem is the database: folders are folders, notes are ordinary `.md` files, and Markerup does not introduce a proprietary note format, database, or sync layer.

The target is one Rust codebase for Linux, Android, and iOS. Linux is the first implemented platform. The core `Workspace` API intentionally uses opaque entry IDs rather than leaking Unix paths so Android Storage Access Framework and iOS document-provider backends can be added without changing the note model.

## Linux workspace features

- Starts with no workspace unless the user explicitly pinned one previously.
- **Choose Folder** opens a native folder picker; no CLI path is required.
- **Pin Workspace** makes the selected workspace reopen on the next launch; unpinned workspaces are session-only.
- Hidden directories such as `.git` are excluded from the notes tree.
- Recursively browse `.md` notes in a collapsible directory tree.
- Create, rename, and delete notes and directories directly on disk.
- Double-confirm destructive deletes.
- Edit the real Markdown files in place.
- Source, preview, and split views.
- `Ctrl+S` save and dirty-state tracking.
- Automatic recursive filesystem watching plus periodic reconciliation for network-backed filesystems.
- External-change reload when the note is clean.
- Conflict protection when a note changes externally while Markerup has unsaved edits.
- Explicit **Use Disk** and **Overwrite Disk** conflict resolution.
- Standard relative Markdown navigation including parent paths, URL-encoded names, and heading fragments.
- Back/forward note navigation.
- Workspace-wide filename/content search.
- Find-next inside the current note.
- Relative Markdown images are resolved inside the workspace and displayed in preview.
- Mermaid fenced diagrams are rendered natively to SVG with Rust and displayed in preview.
- No import/export step and no proprietary metadata required to recover the notes.

## Run on Linux

```bash
cargo run
```

Choose the notes directory from Markerup itself. If you pin it, Markerup will reopen that workspace next time.

On Ubuntu/Kubuntu you may need:

```bash
sudo apt install libfontconfig1-dev
```

Then:

```bash
cargo test
cargo run
```

## Storage invariant

Markerup application state is disposable. Notes and organization live only in the selected filesystem tree. An unpinned workspace is not persisted. Pinning stores only enough application state to reopen the selected workspace and current note.

## Architecture

The shared core uses opaque entry IDs and supports enumeration, read/write, create/rename/delete, search, and relative Markdown link/asset resolution. `LocalWorkspace` is the Linux implementation; its IDs happen to be slash-separated relative paths.

Workspace selection is deliberately platform-specific rather than pretending every OS exposes folders as normal paths:

- **Linux:** native XDG portal folder picker, then normal filesystem access.
- **Android:** Storage Access Framework document-tree selection and persistable URI permissions.
- **iOS:** `UIDocumentPickerViewController` directory selection, security-scoped bookmarks, and coordinated file access.

For iOS, the intended implementation follows Apple's supported external-directory model so a directory exposed by the Files app — including an SMB location — can be selected recursively and bookmarked. See `docs/ios-workspaces.md`.

## Current limitations

- Android and iOS workspace adapters are not implemented yet.
- Mermaid rendering currently uses the Rust-native Merman `0.7.0-alpha.1` release because Merman `0.8.0-alpha.5` requires a newer Rust toolchain than the project currently uses.
- Slint's stock `TextEdit` does not expose per-range text styling, so the editable source pane is monospace but does not yet provide true syntax highlighting. The rendered preview is Markdown-aware.
- Heading-fragment navigation selects the matching heading in the source editor; preview scrolling to an anchor is not yet exposed by Slint's `StyledText`.
