# iOS workspace design

Markerup must treat an iOS workspace as a user-granted document-provider directory, not as a Unix path that happens to be reachable from the sandbox.

## Implemented selection flow

1. Present `UIDocumentPickerViewController` configured for directory selection.
2. Accept the returned security-scoped URL for the selected directory.
3. Call `startAccessingSecurityScopedResource()` before touching the directory.
4. Create bookmark data for the selected URL when the user pins the workspace.
5. On a later launch, resolve the bookmark and re-enter the security scope.
6. Use `NSFileCoordinator` for reads, writes, creates, renames, and deletes to provider-backed content.
7. Release security-scoped access when the workspace is closed or the application no longer needs it.

This is intentionally different from the Linux implementation. The shared `Workspace` API uses opaque entry IDs so provider items do not have to masquerade as ordinary local `PathBuf`s.

The implementation is split between `src/ios_workspace.rs`, `src/ios_bridge.rs`, and `ios/MarkerupIOSBridge.m`. The Tauri frontend is shared with desktop; platform-specific selection and credential storage remain behind Rust commands.

## SMB acceptance target

The device acceptance test is specifically an SMB share mounted through Apple's Files app:

1. Connect the SMB server in Files.
2. Launch Markerup with no workspace selected.
3. Choose Folder and navigate to the SMB-backed notes directory.
4. Verify nested Markdown files enumerate correctly.
5. Edit and save a note, then verify the same bytes from Linux on the server.
6. Create, rename, and delete a note/folder from Markerup and verify those operations on the server.
7. Follow a relative `.md` link to another note.
8. Pin the workspace, force-quit Markerup, relaunch, and verify the security-scoped bookmark restores access without manually navigating back to the share.
9. Modify a note externally and verify Markerup reconciles it when foregrounded/refreshed.
10. Revoke Files/Folders permission and verify Markerup reports loss of access rather than silently showing an empty workspace.

Apple's File Provider/SMB implementation remains outside Markerup's control, so this must be verified on a real device. Markerup must fail visibly on provider-access errors; an inaccessible provider must never be treated as an empty notes directory.
