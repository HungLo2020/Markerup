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

async function launch(snapshot: Record<string, unknown>, contents = "# Note") {
  document.body.innerHTML = '<div id="app"></div>';
  vi.resetModules();
  invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    if (command === "workspace_snapshot") return snapshot;
    if (command === "reload_note") return { id: snapshot.currentFile, contents, snapshot };
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

test.each([false, true])("Insert opens the reminder composer on desktop/mobile (mobile=%s)", async mobile => {
  window.matchMedia = () => ({ matches: mobile, addListener: vi.fn(), removeListener: vi.fn() }) as unknown as MediaQueryList;
  await launch({ ...base, entries: [{ id: "Top.md", name: "Top.md", kind: "File", depth: 0 }], currentFile: "Top.md" });
  document.querySelector<HTMLButtonElement>("#insert")!.click();
  await vi.waitFor(() => expect(document.querySelector(".modal-actions")).not.toBeNull());
  Array.from(document.querySelectorAll<HTMLButtonElement>(".modal-actions button")).find(button => button.textContent === "Reminder")!.click();
  await vi.waitFor(() => expect(document.querySelector(".reminder-form")).not.toBeNull());
  expect(document.querySelector(".modal h2")!.textContent).toBe("Create reminder");
  document.querySelector<HTMLButtonElement>(".modal-close")!.click();
  expect(document.querySelector(".modal-overlay")).toBeNull();
});


test.each([false, true])("editing a reminder updates only its Unicode source range (mobile=%s)", async mobile => {
  window.matchMedia = () => ({ matches: mobile, addListener: vi.fn(), removeListener: vi.fn() }) as unknown as MediaQueryList;
  const source = "- [ ] 日本語 😀 Call @remind(2027-10-02 09:00; tz=UTC; id=stable) trailing text";
  const snap = { ...base, entries: [{ id: "Top.md", name: "Top.md", kind: "File", depth: 0 }], currentFile: "Top.md" };
  await launch(snap, source);
  const original = invoke.getMockImplementation()!;
  const marker = source.indexOf("@remind("), end = source.indexOf(")") + 1;
  const byteOffset = (i: number) => new TextEncoder().encode(source.slice(0,i)).length;
  invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    if (command === "reminder_definitions") return [{id:"stable",title:"日本語 😀 Call",offset:byteOffset(marker),end:byteOffset(end),schedule:{start:"2027-10-02T09:00:00",tz:"UTC",repeat:"once",every:1,weekdays:[],last_day:false}}];
    if (command === "reminder_validate") return "2027-10-02T10:00:00Z";
    if (command === "reminder_permissions") return "Allowed";
    if (command === "save_note") return snap;
    return original(command,args);
  });
  document.querySelector<HTMLButtonElement>("#insert")!.click();
  await vi.waitFor(() => expect(document.querySelector(".modal-actions")).not.toBeNull());
  Array.from(document.querySelectorAll<HTMLButtonElement>(".modal-actions button")).find(button => button.textContent === "Edit reminder on this line")!.click();
  await vi.waitFor(() => expect(document.querySelector(".reminder-form")).not.toBeNull());
  document.querySelector<HTMLInputElement>('.reminder-form [name="time"]')!.value = "10:00";
  document.querySelector<HTMLFormElement>(".reminder-form")!.requestSubmit();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("save_note", {contents:source.replace("09:00", "10:00"),force:false}));
  expect(document.querySelector(".modal-overlay")).toBeNull();
});

test("reminder list opens the associated note and places the cursor at its marker", async () => {
  const snap = { ...base, entries: [{ id: "Top.md", name: "Top.md", kind: "File", depth: 0 }], currentFile: "Top.md" };
  await launch(snap);
  const source = "日本語 😀 Call @remind(2027-10-02 09:00; tz=UTC; id=stable)";
  const position = source.indexOf("@remind");
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    if (command === "reminder_status") return {platform:{message:"Running",limited_until:"2027-10-01T00:00:00Z"},errors:[],items:[{key:"key",id:"stable",file:"Other.md",title:"Call",schedule:"Once"}]};
    if (command === "open_reminder_note") return {id:"Other.md",contents:source,snapshot:{...snap,currentFile:"Other.md",entries:[...snap.entries,{id:"Other.md",name:"Other.md",kind:"File",depth:0}]}};
    if (command === "reminder_definition") return {id:"stable",offset:new TextEncoder().encode(source.slice(0,position)).length};
    return original(command,args);
  });
  document.querySelector<HTMLButtonElement>("#settings")!.click();
  document.querySelector<HTMLButtonElement>("#reminder-settings")!.click();
  await vi.waitFor(() => expect(document.querySelector(".reminder-card")).not.toBeNull());
  Array.from(document.querySelectorAll<HTMLButtonElement>(".reminder-card button")).find(button => button.textContent === "Open note")!.click();
  await vi.waitFor(() => expect(activeId()).toBe("Other.md"));
  const { EditorView } = await import("@codemirror/view");
  const view = EditorView.findFromDOM(document.querySelector(".cm-editor")!)!;
  await vi.waitFor(() => expect(view.state.selection.main.head).toBe(position));
  expect(view.state.doc.toString()).toBe(source);
  const health = document.querySelector<HTMLButtonElement>("#reminder-health")!;
  expect(health.hidden).toBe(false);
  expect(health.tagName).toBe("BUTTON");
  expect(health.textContent).toContain("replenish notifications");
  health.click();
  await vi.waitFor(() => expect(document.querySelector(".reminder-card")).not.toBeNull());
});
