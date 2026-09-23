// @vitest-environment jsdom
import { beforeEach, expect, test, vi } from "vitest";
import { EditorView } from "@codemirror/view";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const base = { workspaceOpen: true, workspacePath: "/notes", workspaceIsSmb: false,
  workspaceFavorited: true, favorites: [], entries: [{ id: "Note.md", name: "Note.md", kind: "File", depth: 0 }],
  currentFile: "Note.md", canGoBack: false, canGoForward: false, externalConflict: false };

beforeEach(() => {
  document.body.innerHTML = '<div id="app"></div>';
  localStorage.clear();
  vi.resetModules();
  window.matchMedia = () => ({ matches: false, addListener: vi.fn(), removeListener: vi.fn() }) as unknown as MediaQueryList;
});

test("restored note and externally changed note load before the next save", async () => {
  let disk = "# Original";
  const saved: string[] = [];
  invoke.mockImplementation(async (command: string, args?: { contents?: string }) => {
    if (command === "workspace_snapshot") return { ...base };
    if (command === "reload_note") return { id: "Note.md", contents: disk, snapshot: { ...base } };
    if (command === "refresh_workspace") return { ...base, externalConflict: true };
    if (command === "save_note") { disk = args!.contents!; saved.push(disk); return { ...base }; }
    throw new Error(`Unexpected command: ${command}`);
  });
  await import("./main");
  await vi.waitFor(() => expect(document.querySelector(".cm-content")?.textContent).toContain("# Original"));

  disk = "# External update";
  (document.querySelector("#refresh") as HTMLButtonElement).click();
  await vi.waitFor(() => expect(document.querySelector(".cm-content")?.textContent).toContain("# External update"));
  expect(saved).toEqual([]);

  const view = EditorView.findFromDOM(document.querySelector(".cm-editor")!);
  expect(view).toBeTruthy();
  view!.dispatch({ changes: { from: view!.state.doc.length, insert: "\nnew text" } });
  expect(localStorage.length).toBe(1);
  await vi.waitFor(() => expect(saved).toEqual(["# External update\nnew text"]), { timeout: 3000 });
  expect(localStorage.length).toBe(0);
});

test("recovered conflicting draft stays unsaved until explicitly resolved", async () => {
  const key = 'markerup-draft-v1:["/notes","Note.md"]';
  localStorage.setItem(key, JSON.stringify({ contents: "# My pending edits", baseline: "# Old disk" }));
  const saves: Array<{ contents: string; force: boolean }> = [];
  invoke.mockImplementation(async (command: string, args?: { contents: string; force: boolean }) => {
    if (command === "workspace_snapshot") return { ...base };
    if (command === "reload_note") return { id: "Note.md", contents: "# New disk", snapshot: { ...base } };
    if (command === "save_note") { saves.push(args!); return { ...base }; }
    throw new Error(`Unexpected command: ${command}`);
  });
  await import("./main");
  await vi.waitFor(() => expect(document.querySelector(".cm-content")?.textContent).toContain("# My pending edits"));
  expect(document.querySelector(".conflict-banner")?.textContent).toContain("changed outside Markerup");
  await new Promise(resolve => setTimeout(resolve, 850));
  expect(saves).toEqual([]);

  (document.querySelector("#overwrite-conflict") as HTMLButtonElement).click();
  await vi.waitFor(() => expect(document.querySelector(".modal-actions .destructive")).not.toBeNull());
  (document.querySelector(".modal-actions .destructive") as HTMLButtonElement).click();
  await vi.waitFor(() => expect(saves).toEqual([{ contents: "# My pending edits", force: true }]));
  expect(localStorage.getItem(key)).toBeNull();
});
