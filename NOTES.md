## Read-only data safety audit

I traced the editor save path, local and SMB workspace operations, backups, trash, session persistence, and iOS document-provider handling. I made no changes and ran no tests.

### Main risks

1. **An external edit can still be overwritten in a narrow timing window.** Before saving, the backend checks that the note still matches the version Markerup opened. It then calls the workspace write operation. If another app changes the note between that check and Markerup’s write, Markerup can replace the external edit. The final read-back confirms Markerup’s text was written, but can’t detect that intervening edit. This applies to local and SMB saves; the pre-check and write are separate operations in [tauri_backend.rs](/home/matt/Documents/Repos/Markerup/src/tauri_backend.rs:607). Backups reduce the impact in some timing cases, but don’t guarantee preservation of every concurrent change.

2. **The native iOS background and resume hooks appear unwired.** The Objective-C bridge defines background/resume observers, and Rust defines functions to install them and consume their signals. I found no call that installs the observers or reads those signals. The frontend does attempt a save on `visibilitychange` and `beforeunload`, but it does not wait for the save to finish. On iOS, the app can be suspended before that asynchronous save completes. Recovery drafts are written synchronously to webview storage as edits occur, which helps, but that storage is still the fallback rather than a confirmed save to the note. The hooks are in [MarkerupIOSBridge.m](/home/matt/Documents/Repos/Markerup/ios/MarkerupIOSBridge.m:589) and [ios_bridge.rs](/home/matt/Documents/Repos/Markerup/src/ios_bridge.rs:46); the app startup path is in [lib.rs](/home/matt/Documents/Repos/Markerup/src/lib.rs:14).

3. **Local deletion can follow a pre-existing `.markerup-trash` symlink.** The local delete path calls `create_dir_all` and then moves the selected file into that location, without rejecting a symlink there. If a workspace already has `.markerup-trash` pointing elsewhere, deleting a note could move it outside the workspace. Backup-directory creation explicitly rejects symlinks; trash creation does not. See [workspace.rs](/home/matt/Documents/Repos/Markerup/src/workspace.rs:618).

4. **Backup history follows the note’s path, not its identity.** The mirrored backup directory is derived from the note’s current relative path. Renaming or moving a note leaves its old history at the old path, and deleting a note leaves its backup history behind. If a different note is later created at that old path, its backup folder can contain snapshots of the earlier note. That matches the path-based storage design, but makes manual recovery less clear. See [backup_directory_id](/home/matt/Documents/Repos/Markerup/src/workspace.rs:29) and the rename/move paths in [workspace.rs](/home/matt/Documents/Repos/Markerup/src/workspace.rs:564).

5. **Trash has no retention or in-app restore flow.** Deletions are moved into `.markerup-trash`, which is safer than permanent deletion, but I found no cleanup policy or restore UI. Deleted notes and folders can accumulate indefinitely. Recovery requires handling hidden workspace files manually, as the [README](/home/matt/Documents/Repos/Markerup/README.md:61) describes.

6. **Recovery drafts depend on webview storage.** The editor stores the current text and save baseline in `localStorage` on each document change. If storage is unavailable or full, Markerup reports that it could not store the draft, but continues editing. If the subsequent save also fails and the app closes, that draft may be lost. The storage and error handling are in [main.ts](/home/matt/Documents/Repos/Markerup/frontend/src/main.ts:35).

### Other limitations

- **Backup pruning is best-effort.** Local, SMB, and iOS pruning errors are ignored after a successful save. If enumeration or deletion fails, backup history can exceed the stated retention policy. The README already calls out provider enumeration limits for iOS.
- **Session preferences are less robust than note saves.** Session state is written directly to its final file, not atomically, and backend persistence errors are ignored. A crash or storage error could lose favorites or the last-open note, but this does not overwrite note content. See [persistence.rs](/home/matt/Documents/Repos/Markerup/src/persistence.rs:76) and [tauri_backend.rs](/home/matt/Documents/Repos/Markerup/src/tauri_backend.rs:274).
- **iOS diagnostics include workspace paths and entry names.** I found no evidence they are transmitted automatically, and the SMB password is not included. Still, logs or copied diagnostics can reveal local folder paths and note/asset filenames. See [MarkerupIOSBridge.m](/home/matt/Documents/Repos/Markerup/ios/MarkerupIOSBridge.m:457).

### Safeguards that are working in the code

- Local saves make a backup before replacing the note, write through a temporary file, and sync the file and parent directory. See [workspace.rs](/home/matt/Documents/Repos/Markerup/src/workspace.rs:499).
- The editor stores a recovery draft on each edit, autosaves after a debounce, retries ordinary failures, and blocks note navigation when it cannot confirm the save.
- SMB saves back up the previous contents and verify the resulting note; ambiguous network outcomes are surfaced instead of blindly retried.
- Deletions are moved to hidden trash, and workspace path validation blocks traversal.
- SMB passwords are omitted from session preferences; iOS uses Keychain for favorited SMB passwords. The session file stores workspace metadata, not the password.

The clearest follow-up priorities are wiring iOS lifecycle reconciliation, narrowing the external-edit race where the platform supports it, and rejecting a symlink at the local trash path.