// @vitest-environment jsdom
import { beforeEach, expect, test, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const entries = [
  { id: "Note.md", name: "Note.md", kind: "File", depth: 0 },
  { id: "Other.md", name: "Other.md", kind: "File", depth: 0 },
];
const base = { workspaceOpen: true, workspacePath: "/notes", workspaceIsSmb: false,
  workspaceFavorited: true, favorites: [], entries,
  currentFile: "Note.md", canGoBack: false, canGoForward: false, externalConflict: false };
const restoring = { ...base, workspaceOpen: false, workspacePath: "", entries: [],
  currentFile: undefined, workspaceRestoring: true };

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

function typeSearch(query: string) {
  const input = document.querySelector<HTMLInputElement>("#search")!;
  input.value = query;
  input.dispatchEvent(new Event("input"));
}

const treeNames = () => Array.from(document.querySelectorAll<HTMLButtonElement>("#tree .entry-main"))
  .map(button => button.dataset.id);

beforeEach(() => {
  document.body.innerHTML = '<div id="app"></div>';
  localStorage.clear();
  vi.resetModules();
  invoke.mockReset();
  Object.defineProperty(Range.prototype, "getClientRects", { configurable: true, value: () => [] });
  Object.defineProperty(Range.prototype, "getBoundingClientRect", { configurable: true, value: () => new DOMRect() });
  window.matchMedia = () => ({ matches: false, addListener: vi.fn(), removeListener: vi.fn() }) as unknown as MediaQueryList;
});

test("search waits for typing to pause and ignores results for older queries", async () => {
  const pending = new Map<string, ReturnType<typeof deferred<string[]>>>();
  invoke.mockImplementation(async (command: string, args?: { query?: string }) => {
    if (command === "workspace_snapshot") return { ...base };
    if (command === "reload_note") return { id: "Note.md", contents: "# Note", snapshot: { ...base } };
    if (command === "preview_document") return { blocks: [] };
    if (command === "search_workspace") {
      const request = deferred<string[]>();
      pending.set(args!.query!, request);
      return request.promise;
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  await import("./main");
  await vi.waitFor(() => expect(document.querySelector("#search")).not.toBeNull());

  typeSearch("n");
  typeSearch("no");
  typeSearch("not");
  await vi.waitFor(() => expect(pending.size).toBe(1));
  expect([...pending.keys()]).toEqual(["not"]);

  typeSearch("oth");
  await vi.waitFor(() => expect(pending.has("oth")).toBe(true));
  pending.get("oth")!.resolve(["Other.md"]);
  await vi.waitFor(() => expect(treeNames()).toEqual(["Other.md"]));

  // The slower, older query must not replace the newer results.
  pending.get("not")!.resolve(["Note.md"]);
  await new Promise(resolve => setTimeout(resolve, 20));
  expect(treeNames()).toEqual(["Other.md"]);
});

test("window is usable while the favorite workspace reopens in the background", async () => {
  const restored = deferred<typeof base | null>();
  invoke.mockImplementation(async (command: string) => {
    if (command === "workspace_snapshot") return { ...restoring };
    if (command === "restored_workspace") return restored.promise;
    if (command === "reload_note") return { id: "Note.md", contents: "# Restored note", snapshot: { ...base } };
    if (command === "preview_document") return { blocks: [] };
    throw new Error(`Unexpected command: ${command}`);
  });
  await import("./main");
  await vi.waitFor(() => expect(document.querySelector("#status")?.textContent).toBe("Reopening favorite workspace…"));
  expect(document.querySelector("#search")).not.toBeNull();

  restored.resolve({ ...base });
  await vi.waitFor(() => expect(document.querySelector(".cm-content")?.textContent).toContain("# Restored note"));
});

test("a reopened favorite does not replace a workspace the user already changed", async () => {
  const restored = deferred<typeof base | null>();
  invoke.mockImplementation(async (command: string) => {
    if (command === "workspace_snapshot") return { ...restoring };
    if (command === "restored_workspace") return restored.promise;
    if (command === "refresh_workspace") return { ...base, currentFile: undefined, workspacePath: "/chosen" };
    if (command === "preview_document") return { blocks: [] };
    throw new Error(`Unexpected command: ${command}`);
  });
  await import("./main");
  await vi.waitFor(() => expect(document.querySelector("#status")?.textContent).toBe("Reopening favorite workspace…"));

  (document.querySelector("#refresh") as HTMLButtonElement).click();
  await vi.waitFor(() => expect(document.querySelector("#status")?.textContent).toBe("Workspace refreshed"));

  restored.resolve({ ...base });
  await new Promise(resolve => setTimeout(resolve, 20));
  expect(invoke.mock.calls.map(([command]) => command)).not.toContain("reload_note");
  expect(document.querySelector("#status")?.textContent).toBe("Workspace refreshed");
});
