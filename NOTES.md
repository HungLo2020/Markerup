Read-only audit complete. I made no changes, commits, pushes, builds, or workflow runs. The worktree remains clean.

Highest-value opportunities:

### Performance and responsiveness

- The frontend recreates the entire page and a new CodeMirror editor whenever notes, workspaces, or settings change. The previous editor is never explicitly destroyed, which can leak listeners, reset history/selection, and cause unnecessary work.  
  [main.ts:102](/home/matt/Documents/Repos/Markerup/frontend/src/main.ts:102), [main.ts:218](/home/matt/Documents/Repos/Markerup/frontend/src/main.ts:218)

- Preview rendering runs on every keystroke and reparses, sanitizes, renders Mermaid, and reloads images. Add preview debouncing, cancellation, and caching.  
  [main.ts:220](/home/matt/Documents/Repos/Markerup/frontend/src/main.ts:220), [main.ts:671](/home/matt/Documents/Repos/Markerup/frontend/src/main.ts:671)

- Live widgets render asynchronously without error handling or stale-render cancellation. A failed image/Mermaid render can leave a blank block or create an unhandled rejection.  
  [main.ts:623](/home/matt/Documents/Repos/Markerup/frontend/src/main.ts:623)

- Workspace refreshes and asset selection perform recursive scans while holding the backend mutex. Large local, iOS, or SMB workspaces can block unrelated commands.  
  [tauri_backend.rs:139](/home/matt/Documents/Repos/Markerup/src/tauri_backend.rs:139), [tauri_backend.rs:533](/home/matt/Documents/Repos/Markerup/src/tauri_backend.rs:533)

- Search rescans the workspace and reads every Markdown file sequentially. The search field also sends a request for every input event, allowing stale results to overwrite newer queries.  
  [workspace.rs:312](/home/matt/Documents/Repos/Markerup/src/workspace.rs:312), [main.ts:531](/home/matt/Documents/Repos/Markerup/frontend/src/main.ts:531)

- Local asset scans have no cancellation, depth limit, or entry limit, unlike SMB scanning. iOS separately enumerates the entire workspace for notes and images.  
  [workspace.rs:225](/home/matt/Documents/Repos/Markerup/src/workspace.rs:225), [ios_workspace.rs:34](/home/matt/Documents/Repos/Markerup/src/ios_workspace.rs:34)

- Local and SMB images are loaded into base64 data URLs repeatedly. A cached asset URL or Tauri asset protocol would reduce memory and network/file reads.  
  [tauri_backend.rs:834](/home/matt/Documents/Repos/Markerup/src/tauri_backend.rs:834)

### Correctness

- Markdown anchors are resolved but then discarded during navigation. Links such as `Note.md#heading` open the note but do not scroll to the heading, despite heading-range support already existing.  
  [tauri_backend.rs:648](/home/matt/Documents/Repos/Markerup/src/tauri_backend.rs:648), [markdown.rs:462](/home/matt/Documents/Repos/Markerup/src/markdown.rs:462)

- SMB link resolution accepts any `.md`-looking path without verifying that the target exists, unlike local workspaces.  
  [smb_workspace.rs:700](/home/matt/Documents/Repos/Markerup/src/smb_workspace.rs:700)

- Supported image extensions and MIME mappings are duplicated across Rust, Objective-C, and TypeScript. These should be centralized or covered by cross-platform tests to prevent drift.

- Folder collapse state is path-based but is not consistently cleaned up after folder rename, deletion, or workspace changes. Stale paths can affect later views.  
  [main.ts:33](/home/matt/Documents/Repos/Markerup/frontend/src/main.ts:33)

### UX and accessibility

- Modals do not trap focus or restore focus when closed.  
  [main.ts:113](/home/matt/Documents/Repos/Markerup/frontend/src/main.ts:113)

- The file tree lacks proper tree semantics such as `role="tree"`, `role="treeitem"`, and meaningful `aria-level`/expanded behavior. File buttons also receive `aria-expanded="undefined"`.  
  [main.ts:205](/home/matt/Documents/Repos/Markerup/frontend/src/main.ts:205)

- Search results show matching notes without their parent folders, making context harder to understand.

- Internal links beginning with `#` are ignored, and external protocol handling only explicitly recognizes HTTP(S) and mail links. Fragment navigation and protocol validation should be made deliberate.

- Live mode hides an entire Markdown block whenever the cursor is anywhere inside it. For multiline lists, quotes, or code blocks, this may feel less intuitive than line-level fallback behavior.

### Maintainability and release quality

- Version numbers are inconsistent: Cargo/Tauri report `0.4.2`, while the frontend package and lockfile report `0.4.0`, and the About screen is hard-coded.  
  [Cargo.toml:3](/home/matt/Documents/Repos/Markerup/Cargo.toml:3), [package.json:4](/home/matt/Documents/Repos/Markerup/frontend/package.json:4)

- `frontend/src/main.ts` is a large monolithic UI module, while the stylesheet is compressed into a few lines. Splitting rendering, editor state, dialogs, workspace tree, and preview logic would make future changes safer.

- The performance documentation claims bounded search caching and cancellation that are not evident in the current workspace implementation.  
  [performance-testing.md:11](/home/matt/Documents/Repos/Markerup/docs/performance-testing.md:11)

- There are Rust unit tests, but no frontend/browser interaction tests. Add coverage for live mode, dropdown selection, image loading, internal-note links, mobile behavior, folder collapse, and save/navigation races.

- Workflow files duplicate substantial setup and hard-code Xcode/iOS versions. The iOS workflow has also emitted setup-toolchain warnings for its comma-separated target input in prior successful runs; that should be made explicit and validated.  
  [ios.yml:21](/home/matt/Documents/Repos/Markerup/.github/workflows/ios.yml:21)

My recommended implementation order would be:

1. Fix editor lifecycle and async preview/live cancellation.
2. Debounce/cache preview, image, and search work.
3. Move workspace scanning off the backend mutex and introduce indexed snapshots.
4. Implement anchor navigation and unify link resolution behavior.
5. Add accessibility improvements and browser-level regression tests.
6. Consolidate versioning and clean up workflow/toolchain configuration.