# Direct SMB workspaces

Markerup supports two folder models:

- Files/iCloud/local folders continue to use the existing platform folder
  picker and filesystem workspace.
- `Connect to SMB` creates a direct SMB2/3 workspace. It connects to the
  server and share supplied by the user, then uses the optional remote folder
  as the workspace root.

The direct backend is shared Rust code on Linux and iOS. The editor, Markdown
preview, search, organization, and file operations use the existing `Workspace`
trait, so they do not depend on how the files are reached. Notes remain normal
`.md` files and directories on the SMB server; Markerup does not create a sync
database or a second canonical copy.

## Security

The password is held in memory for the active connection only. It is not put
in Markdown, logs, diagnostics, session preferences, GitHub Actions, or app
metadata. Direct SMB connections are intentionally session-only in this
version, so the existing “pin workspace” feature is disabled for them. This
avoids accidentally storing a password in the ordinary folder-session format;
a future “remember connection” feature must use iOS Keychain and an equivalent
Linux secret store, never plaintext preferences.

The remote folder and every generated entry ID are checked for absolute paths,
`.`/`..` traversal, NUL bytes, and Windows separator tricks before being sent
to the server.

## Failure behavior and platform differences

SMB connection, authentication, timeout, enumeration, and read failures are
returned to the application as errors. An unavailable share is never converted
to an empty workspace. Reads and enumeration retry once after reconnecting;
mutations are not blindly retried because a network failure can occur after a
server has committed an operation, making an automatic retry ambiguous.

The iOS direct flow does not infer SMB credentials from a Files/LiveFiles URL.
It explicitly asks for server, share, username, password, and remote folder.
This is necessary because the Files provider can expose a URL that opens but
reports a false empty directory, which direct SMB avoids. Linux keeps its
ordinary mounted-folder workflow unchanged.

## Validation

Local validation covers path safety and required connection fields. The Rust
code is checked for `aarch64-apple-ios` and `aarch64-apple-ios-sim` in addition
to the host target. The ignored `real_smb_round_trip` test exercises connect,
enumerate, read, write, create-directory, rename, and delete against a real
server without printing credentials:

```sh
export MARKERUP_SMB_SERVER=server.example
export MARKERUP_SMB_SHARE=Documents
export MARKERUP_SMB_USERNAME=markerup-test
export MARKERUP_SMB_PASSWORD='...'
export MARKERUP_SMB_REMOTE_PATH=Notes
cargo test real_smb_round_trip -- --ignored
```

Use a disposable test directory/share account. The physical-iPhone diagnostic
screen should be used to verify the same direct connection and operations on
the device; it must not be used to display or capture the password.
