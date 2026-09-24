// @vitest-environment jsdom
import { beforeEach, expect, test, vi } from "vitest";
import { EditorView } from "@codemirror/view";
import { readFileSync } from "node:fs";

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
  Object.defineProperty(Range.prototype, "getClientRects", { configurable: true, value: () => [] });
  Object.defineProperty(Range.prototype, "getBoundingClientRect", { configurable: true, value: () => new DOMRect() });
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

test("failed note restore cannot leave an editable blank note", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "workspace_snapshot") return { ...base };
    if (command === "reload_note") throw new Error("provider unavailable");
    throw new Error(`Unexpected command: ${command}`);
  });
  await import("./main");
  await vi.waitFor(() => expect(document.querySelector("#status")?.textContent).toContain("Startup failed"));
  expect(document.querySelector(".cm-content")?.getAttribute("contenteditable")).toBe("false");
  expect(document.querySelector("#insert")).toBeNull();
});

test("content panes get the flexible row beneath toolbar and conflict banner", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "workspace_snapshot") return { ...base };
    if (command === "reload_note") return { id: "Note.md", contents: "# Note", snapshot: { ...base } };
    if (command === "preview_document") return { blocks: [] };
    throw new Error(`Unexpected command: ${command}`);
  });
  await import("./main");
  await vi.waitFor(() => expect(document.querySelector("#panes")).not.toBeNull());
  const documentView = document.querySelector("#document")!;
  expect(Array.from(documentView.children).map(child => child.id || child.className))
    .toEqual(["document-bar", "save-conflict", "panes"]);

  const stylesheet = document.createElement("style");
  stylesheet.textContent = readFileSync("src/styles.css", "utf8");
  document.head.append(stylesheet);
  const mode = document.querySelector<HTMLSelectElement>("#view-mode")!;
  const banner = document.querySelector("#save-conflict")!;
  for (const hasConflict of [false, true]) {
    banner.textContent = hasConflict ? "Conflict" : "";
    for (const value of ["source", "live", "split", "preview"]) {
      mode.value = value;
      mode.dispatchEvent(new Event("change"));
      expect(document.querySelector("#panes")?.className).toBe(value);
      expect(getComputedStyle(documentView).gridTemplateRows.replaceAll(" ", ""))
        .toBe("autoautominmax(0,1fr)");
    }
  }
  stylesheet.remove();
});

test("live mode keeps an unchanged image mounted while editing below it", async () => {
  const imageLine = "![Photo](data:image/png;base64,a)";
  let source = `# Heading\n\n${imageLine}\n\nBelow`;
  invoke.mockImplementation(async (command: string, args?: { source?: string }) => {
    if (command === "workspace_snapshot") return { ...base };
    if (command === "reload_note") return { id: "Note.md", contents: source, snapshot: { ...base } };
    if (command === "preview_document") {
      const start = (args?.source ?? source).indexOf(imageLine);
      return { blocks: [{
        kind: "Image",
        markdown: "Photo",
        image: { alt: "Photo", destination: "data:image/png;base64,a" },
        sourceRange: { start, end: start + imageLine.length },
      }] };
    }
    if (command === "save_note") return { ...base };
    throw new Error(`Unexpected command: ${command}`);
  });

  await import("./main");
  await vi.waitFor(() => expect(document.querySelector(".cm-content")?.textContent).toContain("Below"));
  const mode = document.querySelector<HTMLSelectElement>("#view-mode")!;
  mode.value = "live";
  mode.dispatchEvent(new Event("change"));

  await vi.waitFor(() => expect(document.querySelector(".cm-editor .live-block img")).not.toBeNull());
  const image = document.querySelector(".cm-editor .live-block img");
  const view = EditorView.findFromDOM(document.querySelector(".cm-editor")!);
  expect(view).toBeTruthy();

  const below = source.indexOf("Below");
  view!.dispatch({ selection: { anchor: below + 2 } });
  expect(document.querySelector(".cm-editor .live-block img")).toBe(image);

  view!.dispatch({ changes: { from: below + 2, insert: "!" }, selection: { anchor: below + 3 } });
  source = view!.state.doc.toString();
  expect(document.querySelector(".cm-editor .live-block img")).toBe(image);

  await new Promise(resolve => setTimeout(resolve, 250));
  expect(document.querySelector(".cm-editor .live-block img")).toBe(image);

  const imageOffset = source.indexOf(imageLine);
  view!.dispatch({ selection: { anchor: imageOffset + 5 } });
  expect(document.querySelector(".cm-editor .live-block img")).toBeNull();
  view!.dispatch({ selection: { anchor: source.indexOf("Below") + 2 } });
  await vi.waitFor(() => expect(document.querySelector(".cm-editor .live-block img")).not.toBeNull());
  expect(document.querySelector(".cm-editor .live-block img")).not.toBe(image);
});

test("toggling a live task preserves the editor cursor and saves only the marker change", async () => {
  const taskLine = "- [ ] Task";
  const paragraphs = Array.from({ length: 30 }, (_, index) => `Paragraph ${index}`);
  let source = `${taskLine}\n\n${paragraphs.join("\n\n")}`;
  const saved: string[] = [];
  invoke.mockImplementation(async (command: string, args?: { source?: string; contents?: string; taskOffset?: number }) => {
    if (command === "workspace_snapshot") return { ...base };
    if (command === "reload_note") return { id: "Note.md", contents: source, snapshot: { ...base } };
    if (command === "preview_document") {
      const text = args?.source ?? source;
      const start = text.indexOf("- [");
      return { blocks: [{
        kind: { Task: text.startsWith("- [x]", start) },
        markdown: "Task",
        taskOffset: start,
        sourceRange: { start, end: start + text.slice(start).split("\n")[0]!.length },
      }] };
    }
    if (command === "toggle_markdown_task") {
      return args!.source!.replace(taskLine, "- [x] Task");
    }
    if (command === "save_note") { saved.push(args!.contents!); return { ...base }; }
    throw new Error(`Unexpected command: ${command}`);
  });

  await import("./main");
  await vi.waitFor(() => expect(document.querySelector(".cm-editor")).not.toBeNull());
  const mode = document.querySelector<HTMLSelectElement>("#view-mode")!;
  mode.value = "live";
  mode.dispatchEvent(new Event("change"));
  const view = EditorView.findFromDOM(document.querySelector(".cm-editor")!);
  expect(view).toBeTruthy();
  const cursor = source.indexOf("Paragraph 2") + 4;
  view!.dispatch({ selection: { anchor: cursor } });
  await vi.waitFor(() => expect(document.querySelector<HTMLInputElement>(".cm-editor .live-block input")).not.toBeNull());
  const scroller = document.querySelector<HTMLElement>(".cm-scroller")!;
  scroller.scrollTop = 420;

  const checkbox = document.querySelector<HTMLInputElement>(".cm-editor .live-block input")!;
  checkbox.checked = true;
  checkbox.dispatchEvent(new Event("change"));

  await vi.waitFor(() => expect(saved).toEqual([source.replace(taskLine, "- [x] Task")]));
  expect(view!.state.selection.main.anchor).toBe(cursor);
  expect(scroller.scrollTop).toBe(420);
});

test("note heading shows the filename without its folder or Markdown extension", async () => {
  const file = "Stories/Capital Ships/Resurgence & Friends.md";
  const nested = { ...base, currentFile: file };
  invoke.mockImplementation(async (command: string) => {
    if (command === "workspace_snapshot") return nested;
    if (command === "reload_note") return { id: file, contents: "# Contents", snapshot: nested };
    if (command === "preview_document") return { blocks: [] };
    throw new Error(`Unexpected command: ${command}`);
  });
  await import("./main");
  await vi.waitFor(() => expect(document.querySelector(".note-title")?.textContent).toBe("Resurgence & Friends"));
  const title = document.querySelector<HTMLHeadingElement>(".document-bar h1")!;
  expect(title.title).toBe(file);
  expect(title.innerHTML).toBe("Resurgence &amp; Friends");
  const stylesheet = document.createElement("style");
  stylesheet.textContent = readFileSync("src/styles.css", "utf8");
  document.head.append(stylesheet);
  expect(getComputedStyle(title).fontSize).toBe("32px");
  stylesheet.remove();
});
