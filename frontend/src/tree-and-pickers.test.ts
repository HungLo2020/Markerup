// @vitest-environment jsdom
import { beforeEach, expect, test, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const base = { workspaceOpen: true, workspacePath: "/notes", workspaceIsSmb: false,
  workspaceFavorited: true, favorites: [], canGoBack: false, canGoForward: false, externalConflict: false };
const collapsedKey = "markerup-collapsed-v1:/notes";

const treeIds = () => Array.from(document.querySelectorAll<HTMLButtonElement>("#tree .entry-main"))
  .map(button => button.dataset.id);
const activeId = () => document.querySelector<HTMLButtonElement>("#tree .entry.active .entry-main")?.dataset.id;

async function launch(snapshot: Record<string, unknown>) {
  document.body.innerHTML = '<div id="app"></div>';
  vi.resetModules();
  invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    if (command === "workspace_snapshot") return snapshot;
    if (command === "reload_note") return { id: snapshot.currentFile, contents: "# Note", snapshot };
    if (command === "preview_document") return { blocks: [] };
    if (command === "move_entry") return { ...snapshot, moved: args };
    throw new Error(`Unexpected command: ${command}`);
  });
  await import("./main");
  await vi.waitFor(() => expect(document.querySelector("#tree .entry")).not.toBeNull());
}

beforeEach(() => {
  localStorage.clear();
  invoke.mockReset();
  Object.defineProperty(Range.prototype, "getClientRects", { configurable: true, value: () => [] });
  Object.defineProperty(Range.prototype, "getBoundingClientRect", { configurable: true, value: () => new DOMRect() });
  window.matchMedia = () => ({ matches: false, addListener: vi.fn(), removeListener: vi.fn() }) as unknown as MediaQueryList;
});

test("tree highlights the open note and remembers collapsed folders across launches", async () => {
  const entries = [
    { id: "Folder", name: "Folder", kind: "Directory", depth: 0 },
    { id: "Folder/Inner.md", name: "Inner.md", kind: "File", depth: 1 },
    { id: "Top.md", name: "Top.md", kind: "File", depth: 0 },
  ];
  await launch({ ...base, entries, currentFile: "Top.md" });
  expect(activeId()).toBe("Top.md");
  expect(document.querySelector("#tree .entry.active .entry-main")?.getAttribute("aria-current")).toBe("page");
  expect(document.querySelector('#tree [data-id="Top.md"]')?.hasAttribute("aria-expanded")).toBe(false);

  (document.querySelector('#tree .entry-main[data-id="Folder"]') as HTMLButtonElement).click();
  expect(treeIds()).toEqual(["Folder", "Top.md"]);
  expect(JSON.parse(localStorage.getItem(collapsedKey)!)).toEqual(["Folder"]);

  // Relaunch: the folder stays collapsed.
  await launch({ ...base, entries, currentFile: "Top.md" });
  expect(treeIds()).toEqual(["Folder", "Top.md"]);

  // Opening a note inside a collapsed folder expands the folder to show it.
  await launch({ ...base, entries, currentFile: "Folder/Inner.md" });
  await vi.waitFor(() => expect(activeId()).toBe("Folder/Inner.md"));
  expect(treeIds()).toEqual(["Folder", "Folder/Inner.md", "Top.md"]);
  expect(localStorage.getItem(collapsedKey)).toBeNull();
});

test("move picker filters folders by name and path and picks the first match on Enter", async () => {
  const entries = [
    { id: "Archive", name: "Archive", kind: "Directory", depth: 0 },
    { id: "Archive/Old", name: "Old", kind: "Directory", depth: 1 },
    { id: "Projects", name: "Projects", kind: "Directory", depth: 0 },
    { id: "Top.md", name: "Top.md", kind: "File", depth: 0 },
  ];
  await launch({ ...base, entries, currentFile: "Top.md" });
  (document.querySelector('#tree .entry-actions[data-id="Top.md"]') as HTMLButtonElement).click();
  await vi.waitFor(() => expect(document.querySelector(".modal-actions")).not.toBeNull());
  Array.from(document.querySelectorAll<HTMLButtonElement>(".modal-actions button"))
    .find(button => button.textContent === "Move")!.click();

  const filter = await vi.waitFor(() => {
    const input = document.querySelector<HTMLInputElement>(".picker-filter");
    expect(input).not.toBeNull();
    return input!;
  });
  const choices = () => Array.from(document.querySelectorAll<HTMLButtonElement>(".insert-selector-entry"))
    .map(button => button.textContent);
  expect(choices()).toEqual(["Workspace root", "Archive", "Old", "Projects"]);

  filter.value = "archive old";
  filter.dispatchEvent(new Event("input"));
  // Filtered results show their folder, since the tree indentation is gone.
  expect(choices()).toEqual(["OldArchive"]);

  filter.value = "nothing here";
  filter.dispatchEvent(new Event("input"));
  expect(document.querySelector(".modal .muted")?.textContent).toBe("No matches.");

  filter.value = "old";
  filter.dispatchEvent(new Event("input"));
  filter.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("move_entry", { id: "Top.md", destinationParent: "Archive/Old" }));
  expect(document.querySelector(".modal")).toBeNull();
});
