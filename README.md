# Markerup

Markerup is a filesystem-first Markdown notes editor. The filesystem is the database: folders are folders, notes are ordinary `.md` files, and Markerup does not introduce a proprietary storage or synchronization format.

The long-term target is one Rust codebase for Linux, Android, and iOS. The initial implementation is intentionally Linux-first while keeping filesystem access behind a `Workspace` abstraction so mobile document-provider adapters can be added without changing the note model.

## Current MVP

- Open a directory as a Markdown workspace.
- Recursively discover `.md` files, including nested folders.
- Edit the real files in place.
- Render a live Markdown preview with Slint `StyledText`.
- Save changes explicitly.
- Follow relative Markdown links such as `[Redstone](RedstoneComputer.md)` directly in the preview.
- Refresh the file list after external filesystem changes.
- No database, import step, export step, or sync layer.

## Run on Linux

Install a current Rust toolchain, then run:

```bash
cargo run -- /path/to/your/Notes
```

If no directory is supplied, Markerup uses the current working directory:

```bash
cd ~/Notes
cargo run
```

The first build downloads the Rust dependencies from crates.io. Slint's Linux backend may require your distribution's normal window-system/runtime development packages.

## Architecture

`LocalWorkspace` is the Linux implementation of the `Workspace` trait. All note operations use paths relative to the selected root. Link resolution canonicalizes the target and refuses to navigate outside the workspace.

Future platform work should add document-tree/document-provider implementations rather than changing the core note format:

- Linux: normal filesystem paths.
- Android: Storage Access Framework document trees.
- iOS: document picker/File Provider security-scoped access.

Caches and indexes may be added later, but they must remain disposable. Deleting application state must never destroy or make the Markdown workspace unrecoverable.

## Known MVP limitations

- The workspace is selected from the command line; there is no graphical folder picker yet.
- External file changes require pressing **Refresh**. Filesystem watching is a later milestone.
- Slint `StyledText` currently supports only a subset of CommonMark. Markerup transforms common block constructs into a conservative preview representation; the source Markdown is never rewritten.
- Creating, renaming, moving, and deleting notes/folders are not implemented yet.
- Android and iOS platform adapters are not implemented yet.
