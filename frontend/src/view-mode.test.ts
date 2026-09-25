// @vitest-environment jsdom
import { beforeEach, expect, test, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const snapshot = { workspaceOpen: true, workspacePath: "/notes", workspaceIsSmb: false,
  workspaceFavorited: true, favorites: [], entries: [{ id: "Note.md", name: "Note.md", kind: "File", depth: 0 }],
  currentFile: "Note.md", canGoBack: false, canGoForward: false, externalConflict: false };

async function launch(phoneWidth = false) {
  document.body.innerHTML = '<div id="app"></div>';
  document.body.className = "";
  vi.resetModules();
  window.matchMedia = () => ({ matches: phoneWidth, addListener: vi.fn(), removeListener: vi.fn() }) as unknown as MediaQueryList;
  invoke.mockImplementation(async (command: string) => {
    if (command === "workspace_snapshot") return snapshot;
    if (command === "reload_note") return { id: "Note.md", contents: "# Note", snapshot };
    if (command === "preview_document") return { blocks: [] };
    throw new Error(`Unexpected command: ${command}`);
  });
  await import("./main");
  await vi.waitFor(() => expect(document.querySelector("#view-mode")).not.toBeNull());
}

const mode = () => document.querySelector<HTMLSelectElement>("#view-mode")!;
const shownMode = () => ({ select: mode().value, panes: document.querySelector("#panes")?.className });
function chooseMode(value: string) {
  mode().value = value;
  mode().dispatchEvent(new Event("change"));
}

beforeEach(() => {
  localStorage.clear();
  invoke.mockReset();
  Object.defineProperty(Range.prototype, "getClientRects", { configurable: true, value: () => [] });
  Object.defineProperty(Range.prototype, "getBoundingClientRect", { configurable: true, value: () => new DOMRect() });
});

test("the last chosen view mode is used again after relaunch", async () => {
  await launch();
  expect(shownMode()).toEqual({ select: "split", panes: "split" });

  chooseMode("preview");
  await launch();
  expect(shownMode()).toEqual({ select: "preview", panes: "preview" });

  chooseMode("source");
  await launch();
  expect(shownMode()).toEqual({ select: "source", panes: "source" });
});

test("phone-width layouts remember their own view mode, defaulting to Live", async () => {
  await launch();
  chooseMode("preview");

  await launch(true);
  expect(shownMode()).toEqual({ select: "live", panes: "live" });
  chooseMode("source");

  await launch(true);
  expect(shownMode()).toEqual({ select: "source", panes: "source" });
  await launch();
  expect(shownMode()).toEqual({ select: "preview", panes: "preview" });
});

test("an unrecognised stored view mode falls back to the default", async () => {
  localStorage.setItem("markerup-view-mode-v1:desktop", "fullscreen");
  await launch();
  expect(shownMode()).toEqual({ select: "split", panes: "split" });
});
