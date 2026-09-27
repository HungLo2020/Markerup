import { afterEach, beforeEach, vi } from "vitest";
import { EditorView } from "@codemirror/view";

// Resetting the module registry does not cancel a previous app instance's
// autosave/preview timers. Treat each integration test as a closed webview.
const pending = new Set<ReturnType<typeof setTimeout>>();
let restoreTimer: (() => void) | undefined;
beforeEach(() => {
  if (typeof document === "undefined") return;
  const schedule = globalThis.setTimeout;
  const spy = vi.spyOn(globalThis, "setTimeout").mockImplementation(((...args: Parameters<typeof setTimeout>) => {
    const handle = schedule(...args);
    pending.add(handle);
    return handle;
  }) as typeof setTimeout);
  restoreTimer = () => spy.mockRestore();
});
afterEach(() => {
  if (typeof document !== "undefined") {
    document.querySelectorAll<HTMLElement>(".cm-editor").forEach(node => EditorView.findFromDOM(node)?.destroy());
  }
  for (const handle of pending) clearTimeout(handle);
  pending.clear();
  restoreTimer?.();
  restoreTimer = undefined;
});
