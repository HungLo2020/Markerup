# Markerup

Markerup is a filesystem-first Markdown notes editor. The filesystem is the database: folders are folders, notes are ordinary `.md` files, and Markerup does not introduce a proprietary note format, database, or sync layer.

The target is one Rust codebase for Linux, Android, and iOS. Linux is the first implemented platform. The core `Workspace` API intentionally uses opaque entry IDs rather than leaking Unix paths so Android Storage Access Framework and iOS document-provider backends can be added without changing the note model.

## Linux workspace features

- Starts with no workspace unless the user previously selected a favorite workspace.
- **Choose Folder** opens the platform-native folder picker; no CLI path is required.
- **Favorites** stores any number of selected workspaces and lets you reopen them from Location. Workspaces that are not favorites are session-only.
- Hidden directories such as `.git` are excluded from the notes tree.
- Recursively browse `.md` notes in a collapsible directory tree.
- Create, rename, and delete notes and directories directly on disk.
- Double-confirm destructive deletes.
- Edit the real Markdown files in place.
- Source, preview, and split views.
- Debounced autosave and dirty-state tracking.
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
python3 DevUtils/RunTauriDev.py
```

`cargo run` loads the already-built `frontend/dist` bundle, so it works without
a development server. Use the launcher above (or `cargo tauri dev`) when you
want Tauri's frontend watch/rebuild loop.

Choose the notes directory from Markerup itself. Add it to Favorites if you want Markerup to offer it again after relaunch.

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

Markerup application state is disposable. Notes and organization live only in the selected filesystem tree. A non-favorite workspace is not persisted. Favorites store only enough application state to reopen the selected workspace and current note.

## Architecture

The shared Rust core uses opaque entry IDs and supports enumeration, read/write, create/rename/delete, search, and relative Markdown link/asset resolution. Tauri 2 supplies one HTML/CSS/TypeScript shell for desktop and mobile; its frontend never receives filesystem paths for mutation or SMB credentials. CodeMirror 6 provides the editor surface, including native browser/iOS selection and IME behavior.

Workspace selection is deliberately platform-specific rather than pretending every OS exposes folders as normal paths:

- **Linux:** Tauri's native dialog picker, then normal filesystem access.
- **Android:** Storage Access Framework document-tree selection and persistable URI permissions.
- **iOS:** `UIDocumentPickerViewController` directory selection, security-scoped bookmarks, and coordinated file access.

For iOS, the intended implementation follows Apple's supported external-directory model so a directory exposed by the Files app — including an SMB location — can be selected recursively and bookmarked. See `docs/ios-workspaces.md`.

## Current limitations

- Android workspace integration remains planned. iOS now has a native folder picker, security-scoped bookmark persistence, coordinated file access, foreground reconciliation, and a separate mobile UI. The iOS package must be built on macOS with Xcode/Xcodegen.
- Mermaid rendering remains Rust-native through Merman; the UI only displays the generated SVG.
- Android workspace integration remains planned.
