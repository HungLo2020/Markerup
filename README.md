# Markerup

Markerup is a filesystem-first Markdown notes editor. The filesystem is the database: folders are folders, notes are ordinary `.md` files, and Markerup does not introduce a proprietary note format, database, or sync layer.

The target is one Rust codebase for Linux, Android, and iOS. Linux is the first implemented platform. The core `Workspace` API intentionally uses opaque entry IDs rather than leaking Unix paths so Android Storage Access Framework and iOS document-provider backends can be added without changing the note model.

## Linux workspace features

- Recursively browse `.md` notes in a collapsible directory tree.
- Create, rename, and delete notes and directories directly on disk.
- Double-confirm destructive deletes.
- Edit the real Markdown files in place.
- Source, preview, and split views.
- `Ctrl+S` save and dirty-state tracking.
- Automatic recursive filesystem watching.
- External-change reload when the note is clean.
- Conflict protection when a note changes externally while Markerup has unsaved edits.
- Explicit **Use Disk** and **Overwrite Disk** conflict resolution.
- Standard relative Markdown navigation including parent paths, URL-encoded names, and heading fragments.
- Back/forward note navigation.
- Workspace-wide filename/content search.
- Find-next inside the current note.
- Relative Markdown images are resolved inside the workspace and displayed in preview.
- Remember the last workspace and selected note on Linux.
- No import/export step and no proprietary metadata required to recover the notes.

## Run on Linux

```bash
cargo run -- /path/to/your/Notes
```

After the first run, launching without an argument reopens the last valid workspace:

```bash
cargo run
```

If no saved workspace exists, Markerup uses the current directory.

On Ubuntu/Kubuntu you may need:

```bash
sudo apt install libfontconfig1-dev
```

Then:

```bash
cargo test
cargo run -- /path/to/your/Notes
```

## Storage invariant

Markerup application state is disposable. Notes and organization live only in the selected filesystem tree. The only persisted application state currently stored outside the workspace is the last workspace/current-note pointer under the user's XDG config directory.

## Architecture

The shared core uses opaque entry IDs and supports enumeration, read/write, create/rename/delete, search, and relative Markdown link/asset resolution. `LocalWorkspace` is the Linux implementation; its IDs happen to be slash-separated relative paths.

Future adapters should map native identifiers into the same interface:

- **Linux:** normal filesystem paths.
- **Android:** Storage Access Framework document-tree IDs/URIs.
- **iOS:** document picker/File Provider security-scoped URLs/bookmarks.

The UI and Markdown logic should not assume that an entry ID is a native filesystem path.

## Current limitations

- Android and iOS workspace adapters are not implemented yet.
- The initial workspace is still chosen by CLI path; after that, Markerup remembers it.
- Slint's stock `TextEdit` does not expose per-range text styling, so the editable source pane is monospace but does not yet provide true syntax highlighting. The rendered preview is Markdown-aware.
- Heading-fragment navigation selects the matching heading in the source editor; preview scrolling to an anchor is not yet exposed by Slint's `StyledText`.
