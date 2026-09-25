import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import { EditorState, StateEffect, StateField } from "@codemirror/state";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { Decoration, drawSelection, keymap, EditorView, WidgetType, type DecorationSet } from "@codemirror/view";
import DOMPurify from "dompurify";
import { footnoteBody, footnoteDomId, renderInlineMarkdownHtml, renderMarkdownHtml } from "./markdown-renderer";
import { createUtf8OffsetMapper } from "./markdown-offsets";
import "./styles.css";

const menuIcon = new URL("../../resources/icon_menu.svg", import.meta.url).href;
const settingsIcon = new URL("../../resources/icon_preview_settings.svg", import.meta.url).href;
const folderIcon = new URL("../../resources/icon_folder.svg", import.meta.url).href;

type Entry = { id: string; name: string; kind: "File" | "Directory"; depth: number };
type Favorite = { index: number; label: string; workspaceIsSmb: boolean };
type Snapshot = { workspaceOpen: boolean; workspacePath: string; workspaceIsSmb: boolean; workspaceFavorited: boolean; favorites: Favorite[]; entries: Entry[]; currentFile?: string; canGoBack: boolean; canGoForward: boolean; externalConflict: boolean; workspaceRestoring?: boolean };
type Note = { id: string; contents: string; snapshot: Snapshot };
type SourceRange = { start: number; end: number };
type Block = { kind: unknown; markdown: string; taskOffset?: number; sourceRange?: SourceRange; image?: { alt: string; destination: string }; language?: string; footnoteId?: string };

let snapshot: Snapshot | undefined;
let currentText = "";
let savedText = "";
let saveTimer: number | undefined;
let saveInFlight: Promise<void> | undefined;
let retryTimer: number | undefined;
let saveBlockedUntilReload = false;
let iosWasBackgrounded = false;
const draftPrefix = "markerup-draft-v1:";
function draftKey(note = snapshot?.currentFile, workspace = snapshot?.workspacePath) {
  return note && workspace ? draftPrefix + JSON.stringify([workspace, note]) : undefined;
}
function storeDraft() {
  const key = draftKey();
  if (!key) return;
  try {
    if (currentText === savedText) localStorage.removeItem(key);
    else localStorage.setItem(key, JSON.stringify({ contents: currentText, baseline: savedText }));
  } catch (error) { status(`Could not store recovery draft: ${error}`); }
}
let editor: EditorView | undefined;
let editorState: EditorState | undefined;
let previewTimer: number | undefined;
let page: "main" | "settings" | "location" | "smb" | "about" = "main";
let editorMode: "source" | "live" | "split" | "preview" = "split";
const collapsedDirectories = new Set<string>();
let previewGeneration = 0;
let latestBlocks: Block[] = [];
let latestLinkDefinitions: string[] = [];
const markdownHtmlCache = new Map<string, string>();
let offsetMapperSource = "";
let offsetMapper = createUtf8OffsetMapper("");
const mermaidCache = new Map<string, Promise<string>>();
const assetSourceCache = new Map<string, Promise<string>>();
const MAX_RENDER_CACHE_ENTRIES = 64;
const SEARCH_DELAY_MS = 250;
let searchTimer: number | undefined;
let searchGeneration = 0;

const setLiveDecorations = StateEffect.define<DecorationSet>();
const liveDecorations = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, transaction) {
    let next = value.map(transaction.changes);
    for (const effect of transaction.effects) {
      if (effect.is(setLiveDecorations)) next = effect.value;
    }
    return next;
  },
  provide: field => EditorView.decorations.from(field),
});

const app = document.querySelector<HTMLDivElement>("#app")!;
const status = (message: string) => document.querySelector<HTMLElement>("#status")!.textContent = message;
const escape = (value: string) => value.replace(/[&<>"']/g, char => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[char]!));
const noteTitle = (path: string) => path.split(/[\\/]/).pop()!.replace(/\.md$/i, "");
const call = <T>(command: string, args?: Record<string, unknown>) => invoke<T>(command, args);
const mobileLayout = () => window.matchMedia("(max-width: 700px)").matches;
// iPadOS can present a desktop-style user agent, so include its touch-capable
// MacIntel form as well as the conventional iOS device identifiers. This must
// be a platform check rather than a viewport check: desktop mobile-preview
// windows still use Tauri's desktop dialog plugin.
const iosDevice = () => /iPad|iPhone|iPod/.test(navigator.userAgent)
  || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);

function cancelScheduledPreview() {
  previewGeneration += 1;
  if (previewTimer !== undefined) {
    clearTimeout(previewTimer);
    previewTimer = undefined;
  }
}

function schedulePreview(delay = 150) {
  cancelScheduledPreview();
  const generation = previewGeneration;
  previewTimer = window.setTimeout(() => {
    previewTimer = undefined;
    void refreshPreview(generation);
  }, delay);
}

function renderShell() {
  disposeEditor();
  const headerLocation = snapshot?.currentFile
    ? noteTitle(snapshot.currentFile)
    : snapshot?.workspacePath ?? "No workspace";
  app.innerHTML = `<header><button id="menu" class="icon-button" aria-label="Toggle workspace"><img src="${menuIcon}" alt=""></button><strong>Markerup</strong><span id="location">${escape(headerLocation)}</span><span class="grow"></span><button id="back">←</button><button id="forward">→</button><button id="refresh">Refresh</button><button id="settings" class="icon-button" aria-label="Settings"><img src="${settingsIcon}" alt=""></button></header><main id="content"></main><footer id="status">Ready</footer>`;
  document.querySelector("#menu")!.addEventListener("click", () => document.body.classList.toggle("sidebar-hidden"));
  document.querySelector("#settings")!.addEventListener("click", () => { page = "settings"; renderPage(); });
  document.querySelector("#refresh")!.addEventListener("click", refresh);
  document.querySelector("#back")!.addEventListener("click", () => navigate("go_back"));
  document.querySelector("#forward")!.addEventListener("click", () => navigate("go_forward"));
}

function cancelSearch() {
  searchGeneration += 1;
  if (searchTimer !== undefined) {
    clearTimeout(searchTimer);
    searchTimer = undefined;
  }
}

function renderPage() {
  cancelScheduledPreview();
  // The sidebar and its search box are rebuilt below; results for the old
  // query must not filter the new tree.
  cancelSearch();
  latestBlocks = [];
  disposeEditor();
  const content = document.querySelector<HTMLElement>("#content")!;
  if (page === "settings") { content.innerHTML = panel("Settings", `<button id="location-settings">Location</button><button id="about">About</button>`); document.querySelector("#location-settings")!.addEventListener("click",()=>{page="location";renderPage()}); document.querySelector("#about")!.addEventListener("click",()=>{page="about";renderPage()}); return; }
  if (page === "location") {
    const favorites = snapshot?.favorites ?? [];
    const favoriteList = favorites.length
      ? favorites.map(favorite => `<button class="favorite-workspace" data-index="${favorite.index}">${favorite.workspaceIsSmb ? "SMB · " : "Local · "}${escape(favorite.label)}</button>`).join("")
      : `<p class="muted">No favorite workspaces yet.</p>`;
    content.innerHTML = panel("Location", `<p>${snapshot?.workspaceOpen ? `${snapshot.workspaceIsSmb ? "SMB" : "Local"} workspace: ${escape(snapshot.workspacePath)}` : "No workspace selected."}</p><button id="browse">Browse local folders</button><button id="smb">Connect to SMB</button><button id="favorite">${snapshot?.workspaceFavorited ? "Remove from Favorites" : "Add to Favorites"}</button><h2>Favorites</h2><div class="favorites-list">${favoriteList}</div>`);
    document.querySelector("#browse")!.addEventListener("click", chooseLocal);
    document.querySelector("#smb")!.addEventListener("click",()=>{page="smb";renderPage()});
    document.querySelector("#favorite")!.addEventListener("click",toggleFavorite);
    document.querySelectorAll<HTMLButtonElement>(".favorite-workspace").forEach(button => button.addEventListener("click", () => openFavorite(Number(button.dataset.index))));
    return;
  }
  if (page === "smb") { content.innerHTML = panel("Connect to SMB", `<label>Server<input id="server" placeholder="server or IP"></label><label>Share<input id="share"></label><label>Username<input id="username"></label><label>Password<input id="password" type="password"></label><label>Remote folder<input id="remote" placeholder="Notes"></label><button id="connect">Connect</button>`); document.querySelector("#connect")!.addEventListener("click",connectSmb); return; }
  if (page === "about") { content.innerHTML = panel("About Markerup", `<p>Version ${__MARKERUP_VERSION__}</p><button id="privacy">Privacy Policy</button>`); document.querySelector("#privacy")!.addEventListener("click",async()=>openExternal(await call<string>("privacy_policy_url"))); return; }
  const viewControls = `<select id="view-mode" aria-label="Editor view">${[
    ["source", "Source"],
    ["live", "Live"],
    ["split", "Split"],
    ["preview", "Preview"],
  ].map(([value, label]) => `<option value="${value}"${editorMode === value ? " selected" : ""}>${label}</option>`).join("")}</select>`;
  const currentFile = snapshot?.currentFile;
  content.innerHTML = `<aside id="sidebar"><div class="row"><strong>Workspace</strong><button id="new" aria-label="Create">＋</button></div><input id="search" placeholder="Search all notes"><nav id="tree"></nav></aside><section id="document"><div class="document-bar"><h1 class="note-title"${currentFile ? ` title="${escape(currentFile)}"` : ""}>${escape(currentFile ? noteTitle(currentFile) : "Choose a note")}</h1><span class="grow"></span>${currentFile ? `<button id="insert">Insert</button>` : ""}${viewControls}</div><div id="save-conflict" role="alert"></div><div id="panes"><div id="editor-pane"><div id="editor"></div></div><article id="preview"></article></div></section>`;
  document.querySelector("#new")!.addEventListener("click",()=>createAtRoot());
  document.querySelector("#search")!.addEventListener("input", scheduleSearch);
  document.querySelector("#insert")?.addEventListener("click", () => void showInsertMenu());
  document.querySelector<HTMLSelectElement>("#view-mode")!.addEventListener("change", event => {
    editorMode = (event.target as HTMLSelectElement).value as typeof editorMode;
    applyMode();
    if (editorMode === "live") schedulePreview(0);
  });
  renderTree(); setupEditor(); showConflict(); schedulePreview(0); applyMode();
}
function showConflict() {
  const host = document.querySelector<HTMLElement>("#save-conflict");
  if (!host) return;
  if (!saveBlockedUntilReload && !snapshot?.externalConflict) { host.replaceChildren(); return; }
  host.innerHTML = `<div class="conflict-banner">The note changed outside Markerup or its save could not be verified. Your edits are still in this editor. <button id="copy-conflict">Copy my text</button><button id="reload-conflict">Use disk version</button><button id="overwrite-conflict">Overwrite disk</button></div>`;
  host.querySelector("#copy-conflict")!.addEventListener("click", async () => {
    try { await navigator.clipboard.writeText(currentText); status("Editor text copied"); }
    catch (error) { status(`Copy failed: ${error}`); }
  });
  host.querySelector("#reload-conflict")!.addEventListener("click", async () => {
    if (currentText !== savedText && !await confirmAction("Discard editor changes", "Discard my changes and load the disk version?")) return;
    try { loadNote(await call<Note>("reload_note"), true); }
    catch (error) { status(`Reload failed: ${error}`); }
  });
  host.querySelector("#overwrite-conflict")!.addEventListener("click", async () => {
    if (!await confirmAction("Overwrite external changes", "Replace the disk version with my editor text?")) return;
    try {
      const contents = currentText;
      snapshot = await call<Snapshot>("save_note", { contents, force: true });
      savedText = contents;
      storeDraft();
      saveBlockedUntilReload = false;
      showConflict();
      status("Saved");
      if (currentText !== savedText) scheduleSave();
    } catch (error) { saveBlockedUntilReload = true; showConflict(); status(`Save failed: ${error}`); }
  });
}
function panel(title:string, body:string) { return `<section class="panel"><button id="panel-back">← Back</button><h1>${title}</h1>${body}</section>`; }
document.addEventListener("click", event => {
  if ((event.target as HTMLElement).id !== "panel-back") return;
  page = page === "settings" ? "main" : "settings";
  renderPage();
});

type ModalAction = { id: string; label: string; destructive?: boolean };

function modalSurface<T>(title: string, close: (value?: T) => void) {
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  overlay.innerHTML = `<section class="modal" role="dialog" aria-modal="true" aria-labelledby="modal-title"><div class="modal-header"><h2 id="modal-title">${escape(title)}</h2><button class="modal-close" aria-label="Close">×</button></div><div class="modal-body"></div></section>`;
  let dismissed = false;
  const dismiss = (value?: T) => {
    if (dismissed) return;
    dismissed = true;
    window.removeEventListener("keydown", onKeyDown);
    overlay.remove();
    close(value);
  };
  const onKeyDown = (event: KeyboardEvent) => { if (event.key === "Escape") dismiss(); };
  overlay.addEventListener("click", event => { if (event.target === overlay) dismiss(); });
  overlay.querySelector<HTMLButtonElement>(".modal-close")!.addEventListener("click", () => dismiss());
  window.addEventListener("keydown", onKeyDown);
  document.body.append(overlay);
  return { body: overlay.querySelector<HTMLElement>(".modal-body")!, dismiss };
}

function chooseAction(title: string, actions: ModalAction[]): Promise<string | undefined> {
  return new Promise(resolve => {
    const modal = modalSurface<string>(title, resolve);
    const choices = document.createElement("div");
    choices.className = "modal-actions";
    for (const action of actions) {
      const button = document.createElement("button");
      button.textContent = action.label;
      if (action.destructive) button.classList.add("destructive");
      button.addEventListener("click", () => modal.dismiss(action.id));
      choices.append(button);
    }
    modal.body.append(choices);
  });
}

function requestName(title: string, initialValue = ""): Promise<string | undefined> {
  return new Promise(resolve => {
    const modal = modalSurface<string>(title, resolve);
    const form = document.createElement("form");
    form.className = "modal-form";
    form.innerHTML = `<label>Name<input name="name" autocomplete="off" required></label><div class="modal-actions"><button type="button" class="secondary">Cancel</button><button type="submit">Continue</button></div>`;
    const input = form.elements.namedItem("name") as HTMLInputElement;
    input.value = initialValue;
    form.querySelector<HTMLButtonElement>(".secondary")!.addEventListener("click", () => modal.dismiss());
    form.addEventListener("submit", event => {
      event.preventDefault();
      const name = input.value.trim();
      if (!name) { input.focus(); return; }
      modal.dismiss(name);
    });
    modal.body.append(form);
    requestAnimationFrame(() => input.focus());
  });
}

function requestValue(title: string, label: string, initialValue = "", type = "text"): Promise<string | undefined> {
  return new Promise(resolve => {
    const modal = modalSurface<string>(title, resolve);
    const form = document.createElement("form");
    form.className = "modal-form";
    form.innerHTML = `<label>${escape(label)}<input name="value" type="${type}" autocomplete="off" required></label><div class="modal-actions"><button type="button" class="secondary">Cancel</button><button type="submit">Insert</button></div>`;
    const input = form.elements.namedItem("value") as HTMLInputElement;
    input.value = initialValue;
    form.querySelector<HTMLButtonElement>(".secondary")!.addEventListener("click", () => modal.dismiss());
    form.addEventListener("submit", event => {
      event.preventDefault();
      const value = input.value.trim();
      if (!value) { input.focus(); return; }
      modal.dismiss(value);
    });
    modal.body.append(form);
    requestAnimationFrame(() => input.focus());
  });
}

async function confirmAction(title: string, message: string): Promise<boolean> {
  const answer = await chooseAction(title, [{ id: "cancel", label: "Cancel" }, { id: "confirm", label: message, destructive: true }]);
  return answer === "confirm";
}

function visibleTreeEntries(entries: Entry[]) {
  const visible: Entry[] = [];
  for (const entry of entries) {
    const parts = entry.id.split("/");
    const hidden = parts.slice(0, -1).some((_, index) => collapsedDirectories.has(parts.slice(0, index + 1).join("/")));
    if (!hidden) visible.push(entry);
  }
  return visible;
}
function renderTree(entries = snapshot?.entries ?? []) {
  const tree=document.querySelector("#tree"); if (!tree) return;
  tree.innerHTML=visibleTreeEntries(entries).map(entry=>`<div class="entry" style="padding-left:${entry.depth * 16 + 6}px"><button class="entry-main" data-id="${escape(entry.id)}" aria-expanded="${entry.kind === "Directory" ? !collapsedDirectories.has(entry.id) : undefined}">${entry.kind === "Directory" ? `<span class="entry-disclosure">${collapsedDirectories.has(entry.id) ? "▸" : "▾"}</span><img class="entry-folder-icon" src="${folderIcon}" alt="">` : "·"} ${escape(entry.name)}</button><button class="entry-actions" data-id="${escape(entry.id)}" data-kind="${entry.kind}">…</button></div>`).join("") || "<p class=muted>No Markdown notes found.</p>";
  tree.querySelectorAll<HTMLButtonElement>(".entry-main").forEach(b=>b.addEventListener("click",()=>{
    const entry = entries.find(candidate => candidate.id === b.dataset.id);
    if (entry?.kind === "Directory") {
      if (collapsedDirectories.has(entry.id)) collapsedDirectories.delete(entry.id);
      else collapsedDirectories.add(entry.id);
      renderTree(entries);
      return;
    }
    void openNote(b.dataset.id!);
  }));
  tree.querySelectorAll<HTMLButtonElement>(".entry-actions").forEach(b=>b.addEventListener("click",()=>entryActions(b.dataset.id!, b.dataset.kind === "Directory")));
}
function disposeEditor() {
  if (!editor) return;
  editorState = editor.state;
  editor.destroy();
  editor = undefined;
}
function setupEditor() {
  const host=document.querySelector<HTMLElement>("#editor")!;
  const state = editorState && editorState.doc.toString() === currentText
    ? editorState
  : EditorState.create({ doc: currentText, extensions: [EditorView.editable.of(Boolean(snapshot?.currentFile)), liveDecorations, history(), markdown(), keymap.of([...defaultKeymap,...historyKeymap]), EditorView.lineWrapping, drawSelection({iosSelectionHandles:true}), EditorView.theme({"&":{height:"100%"},".cm-scroller":{overflow:"auto",fontFamily:"inherit",lineHeight:"1.28"},".cm-content":{lineHeight:"1.28",padding:"12px"},".cm-line":{lineHeight:"1.28"},".cm-selectionBackground":{backgroundColor:"rgba(10, 132, 255, 0.30)"},"&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground":{backgroundColor:"rgba(10, 132, 255, 0.52)"}}, {dark:true}), EditorView.updateListener.of(update=>{if(update.docChanged){currentText=update.state.doc.toString();editorState=update.state;storeDraft();scheduleSave();schedulePreview();} else if(update.selectionSet && editorMode === "live"){editorState=update.state;updateLiveDecorations(latestBlocks);}})] });
  editor = new EditorView({ state, parent:host });
  editorState = editor.state;
  if (!iosDevice()) host.addEventListener("contextmenu", event => {
    event.preventDefault();
    void showInsertMenu();
  });
}
function applyMode(){
  const panes=document.querySelector("#panes");
  if(panes) panes.className=editorMode;
  if (editorMode === "live") void updateLiveDecorations(latestBlocks);
  else clearLiveDecorations();
}
async function openNote(id:string){
  if(!await saveBeforeChangingNote()) return;
  try {
    const note=await call<Note>("open_note",{id});
    openNoteView(note);
  } catch(error){ status(`Open failed: ${error}`); }
}
function openNoteView(note: Note) {
  if (mobileLayout()) {
    editorMode="live";
    document.body.classList.add("sidebar-hidden");
  }
  loadNote(note);
}
function loadNote(note:Note, discardDraft = false){
  if(saveTimer) clearTimeout(saveTimer);
  if(retryTimer) clearTimeout(retryTimer);
  saveBlockedUntilReload=false;
  snapshot=note.snapshot;
  currentText=savedText=note.contents;
  const key=draftKey();
  try {
    const raw=key && localStorage.getItem(key);
    if(discardDraft && key) localStorage.removeItem(key);
    else if(raw) {
      const draft=JSON.parse(raw) as {contents:string;baseline:string};
      if(typeof draft.contents === "string" && typeof draft.baseline === "string" && draft.contents !== note.contents) {
        currentText=draft.contents;
        saveBlockedUntilReload=draft.baseline !== note.contents;
      } else if(key) localStorage.removeItem(key);
    }
  } catch(error){status(`Could not restore recovery draft: ${error}`)}
  editorState=undefined; renderShell(); renderPage();
  if(currentText !== savedText && !saveBlockedUntilReload) scheduleSave();
  else status(saveBlockedUntilReload ? "Recovered draft conflicts with disk — resolve before saving" : "Saved");
}
function clearNoteView() {
  if (saveTimer) clearTimeout(saveTimer);
  if (retryTimer) clearTimeout(retryTimer);
  currentText = savedText = "";
  saveBlockedUntilReload = false;
  editorState = undefined;
}
function scheduleSave(delay=750){
  if(saveBlockedUntilReload) {
    status("Reload this note before saving again");
    return;
  }
  status("Unsaved changes");
  if(saveTimer) clearTimeout(saveTimer);
  saveTimer=window.setTimeout(()=>void flushSave(),delay);
}
function scheduleRetry(error: unknown){
  const message=String(error);
  if (/outcome unknown|external change conflict/i.test(message)) {
    saveBlockedUntilReload=true;
    showConflict();
    status(message);
    return;
  }
  status(`Save failed — retrying: ${message}`);
  if(retryTimer) clearTimeout(retryTimer);
  retryTimer=window.setTimeout(()=>void flushSave(),2000);
}
function flushSave(): Promise<void> {
  if(saveInFlight) return saveInFlight;
  if(saveBlockedUntilReload) return Promise.resolve();
  if(retryTimer) { clearTimeout(retryTimer); retryTimer=undefined; }
  saveInFlight=(async()=>{
    while(snapshot?.currentFile && currentText!==savedText) {
      const file=snapshot.currentFile;
      const contents=currentText;
      status("Saving…");
      try {
        const next=await call<Snapshot>("save_note",{contents,force:false});
        if(snapshot?.currentFile!==file) return;
        snapshot=next;
        savedText=contents;
        storeDraft();
        status("Saved");
      } catch(error) {
        scheduleRetry(error);
        return;
      }
    }
  })().finally(()=>{
    saveInFlight=undefined;
    if(snapshot?.currentFile && currentText!==savedText && !retryTimer && !saveBlockedUntilReload) scheduleSave();
  });
  return saveInFlight;
}
async function saveBeforeChangingNote(): Promise<boolean> {
  await flushSave();
  if(currentText===savedText) return true;
  status("Unsaved changes must be resolved before leaving this note");
  return false;
}
async function refresh(){
  await flushSave();
  try {
    assetSourceCache.clear();
    const file = snapshot?.currentFile;
    snapshot=await call<Snapshot>("refresh_workspace",{editorHasUnsavedChanges:currentText!==savedText});
    if (file && snapshot.currentFile === file && currentText === savedText && snapshot.externalConflict) {
      loadNote(await call<Note>("reload_note"));
      status("External changes loaded");
      return;
    }
    renderShell(); renderPage(); status(snapshot.externalConflict ? "External change conflict — resolve before saving" : "Workspace refreshed");
  } catch(error){status(`Refresh failed: ${error}`)}
}
async function navigate(command:string){ if(!await saveBeforeChangingNote()) return; const note=await call<Note|null>(command); if(note) loadNote(note); }
async function chooseLocal(){
  if(!await saveBeforeChangingNote()) return;
  try {
    if (iosDevice()) {
      const selected = await call<Snapshot | null>("choose_ios_workspace");
      if (!selected) return;
      snapshot = selected;
    } else {
      const selected=await openDialog({directory:true,multiple:false});
      if(typeof selected !== "string") return;
      snapshot=await call<Snapshot>("open_local_workspace",{path:selected});
    }
    clearNoteView();
    page="main";
    renderShell();
    renderPage();
    status("Local workspace selected");
  } catch(error) {
    status(`Workspace selection failed: ${error}`);
  }
}
async function connectSmb(){ if(!await saveBeforeChangingNote()) return; const value=(id:string) => document.querySelector<HTMLInputElement>(`#${id}`)!.value; try { snapshot=await call<Snapshot>("connect_smb",{request:{server:value("server"),share:value("share"),username:value("username"),password:value("password"),remotePath:value("remote")}}); clearNoteView(); page="main";renderShell();renderPage();status("SMB workspace connected"); } catch(error){status(`SMB connection failed: ${error}`)} }
async function toggleFavorite(){
  try {
    snapshot=await call<Snapshot>("set_workspace_favorite",{favorited:!snapshot?.workspaceFavorited});
    renderShell();
    renderPage();
    status(snapshot.workspaceFavorited ? "Workspace added to Favorites" : "Workspace removed from Favorites");
  } catch(error) {
    status(String(error));
  }
}
async function openFavorite(index: number) {
  if(!await saveBeforeChangingNote()) return;
  try {
    snapshot=await call<Snapshot>("open_favorite_workspace",{index});
    clearNoteView();
    page="main";
    renderShell();
    renderPage();
    status("Favorite workspace selected");
  } catch(error) {
    status(`Could not open favorite workspace: ${error}`);
  }
}
async function createAtRoot(){
  const type = await chooseAction("Create in workspace", [{ id: "note", label: "New note" }, { id: "folder", label: "New folder" }]);
  if(type === "note") return createEntry("",true);
  if(type === "folder") return createEntry("",false);
}
async function createEntry(parent:string,note:boolean){ if(!await saveBeforeChangingNote()) return; const name=await requestName(note?"New note":"New folder"); if(!name)return; try { if(note){loadNote(await call<Note>("create_note",{parent,name}));} else {snapshot=await call<Snapshot>("create_folder",{parent,name});renderShell();renderPage();} }catch(error){status(String(error))} }
function selectedText() {
  if (!editor) return "";
  const selection = editor.state.selection.main;
  return editor.state.sliceDoc(selection.from, selection.to).trim();
}
function encodeMarkdownPath(path: string) {
  return path.split("/").map(part => encodeURIComponent(part)).join("/");
}
function relativeMarkdownPath(target: string) {
  const current = snapshot?.currentFile ?? "";
  const currentParts = current.split("/");
  currentParts.pop();
  const targetParts = target.split("/");
  while (currentParts.length && targetParts.length && currentParts[0] === targetParts[0]) {
    currentParts.shift();
    targetParts.shift();
  }
  return encodeMarkdownPath([...currentParts.map(() => ".."), ...targetParts].join("/") || ".");
}
function insertAtSelection(text: string) {
  const currentEditor = editor;
  if (!currentEditor) return;
  const selection = currentEditor.state.selection.main;
  currentEditor.dispatch({
    changes: { from: selection.from, to: selection.to, insert: text },
    selection: { anchor: selection.from + text.length },
    userEvent: "input.insert",
  });
  currentEditor.focus();
}
function chooseNoteTarget(): Promise<Entry | undefined> {
  return new Promise(resolve => {
    const modal = modalSurface<Entry>("Insert link to note", resolve);
    const list = document.createElement("div");
    list.className = "insert-selector";
    const notes = (snapshot?.entries ?? []).filter(entry => entry.kind === "File");
    if (!notes.length) {
      list.innerHTML = `<p class="muted">No Markdown notes found.</p>`;
    } else {
      for (const note of notes) {
        const button = document.createElement("button");
        button.className = "insert-selector-entry";
        button.style.paddingLeft = `${note.depth * 16 + 12}px`;
        button.textContent = note.name;
        button.title = note.id;
        button.addEventListener("click", () => modal.dismiss(note));
        list.append(button);
      }
    }
    modal.body.append(list);
  });
}
async function chooseWorkspaceAsset(): Promise<Entry | undefined> {
  try {
    const assets = await call<Entry[]>("workspace_assets");
    return await new Promise(resolve => {
      const modal = modalSurface<Entry>("Insert workspace image", resolve);
      const list = document.createElement("div");
      list.className = "insert-selector";
      if (!assets.length) {
        list.innerHTML = `<p class="muted">No supported images found in this workspace.</p>`;
      } else {
        for (const asset of assets) {
          const button = document.createElement("button");
          button.className = "insert-selector-entry";
          button.style.paddingLeft = `${asset.depth * 16 + 12}px`;
          button.textContent = asset.name;
          button.title = asset.id;
          button.addEventListener("click", () => modal.dismiss(asset));
          list.append(button);
        }
      }
      modal.body.append(list);
    });
  } catch (error) {
    status(`Image list failed: ${error}`);
    return undefined;
  }
}
async function insertNoteLink() {
  const target = await chooseNoteTarget();
  if (!target) return;
  const label = await requestValue("Link text", "Text", selectedText() || target.name.replace(/\.md$/i, ""));
  if (label === undefined) return;
  insertAtSelection(`[${label}](${relativeMarkdownPath(target.id)})`);
}
async function insertUrlLink() {
  const url = await requestValue("Insert web link", "URL", "https://", "url");
  if (url === undefined) return;
  const label = await requestValue("Link text", "Text", selectedText() || url);
  if (label === undefined) return;
  insertAtSelection(`[${label}](${url})`);
}
async function insertWorkspaceImage() {
  const asset = await chooseWorkspaceAsset();
  if (!asset) return;
  const alt = await requestValue("Image description", "Alt text", selectedText() || asset.name.replace(/\.[^.]+$/, ""));
  if (alt === undefined) return;
  insertAtSelection(`![${alt}](${relativeMarkdownPath(asset.id)})`);
}
async function insertUrlImage() {
  const url = await requestValue("Insert web image", "Image URL", "https://", "url");
  if (url === undefined) return;
  const alt = await requestValue("Image description", "Alt text", selectedText() || "Image");
  if (alt === undefined) return;
  insertAtSelection(`![${alt}](${url})`);
}
async function showInsertMenu() {
  if (!snapshot?.currentFile || !editor) return;
  const action = await chooseAction("Insert Markdown", [
    { id: "note-link", label: "Link to note" },
    { id: "web-link", label: "Web link" },
    { id: "workspace-image", label: "Workspace image" },
    { id: "web-image", label: "Web image" },
  ]);
  if (action === "note-link") return insertNoteLink();
  if (action === "web-link") return insertUrlLink();
  if (action === "workspace-image") return insertWorkspaceImage();
  if (action === "web-image") return insertUrlImage();
}
function chooseDestinationFolder(sourceId: string): Promise<string | undefined> {
  return new Promise(resolve => {
    const modal = modalSurface<string>("Move to folder", resolve);
    const list = document.createElement("div");
    list.className = "folder-selector";
    const folders = (snapshot?.entries ?? []).filter(entry => {
      if (entry.kind !== "Directory") return false;
      return entry.id !== sourceId && !entry.id.startsWith(`${sourceId}/`);
    });
    const destinations: Array<{ id: string; label: string; depth: number }> = [{ id: "", label: "Workspace root", depth: 0 }];
    destinations.push(...folders.map(folder => ({ id: folder.id, label: folder.name, depth: folder.depth + 1 })));
    for (const destination of destinations) {
      const button = document.createElement("button");
      button.className = "folder-selector-entry";
      button.style.paddingLeft = `${destination.depth * 16 + 12}px`;
      button.textContent = destination.label;
      button.addEventListener("click", () => modal.dismiss(destination.id));
      list.append(button);
    }
    modal.body.append(list);
  });
}
async function moveEntry(id: string) {
  if(!await saveBeforeChangingNote()) return;
  const destination = await chooseDestinationFolder(id);
  if(destination === undefined) return;
  try {
    snapshot = await call<Snapshot>("move_entry", { id, destinationParent: destination });
    const movedName = id.split("/").pop() ?? id;
    const newId = destination ? `${destination}/${movedName}` : movedName;
    for (const collapsed of [...collapsedDirectories]) {
      if (collapsed === id || collapsed.startsWith(`${id}/`)) {
        collapsedDirectories.delete(collapsed);
        collapsedDirectories.add(`${newId}${collapsed.slice(id.length)}`);
      }
    }
    renderShell();
    renderPage();
    status("Entry moved");
  } catch(error) {
    status(`Move failed: ${error}`);
  }
}
async function entryActions(id:string,isDirectory:boolean){
  const actions: ModalAction[] = [];
  if (isDirectory) actions.push({ id: "new-note", label: "New note" }, { id: "new-folder", label: "New folder" });
  actions.push({ id: "move", label: "Move" }, { id: "rename", label: "Rename" }, { id: "delete", label: "Delete", destructive: true });
  const action = await chooseAction(id.split("/").pop() ?? "Actions", actions);
  if(!action)return;
  if(action==="new-note")return createEntry(id,true);
  if(action==="new-folder")return createEntry(id,false);
  if(action==="move")return moveEntry(id);
  if(!await saveBeforeChangingNote()) return;
  try {
    if(action==="rename"){
      const name=await requestName("Rename", id.split("/").pop() ?? "");
      if(!name)return;
      snapshot=await call<Snapshot>("rename_entry",{id,name});
    }
    if(action==="delete" && await confirmAction("Move to trash", `Move ${id.split("/").pop() ?? "this entry"} to .markerup-trash?`)) {
      snapshot=await call<Snapshot>("delete_entry",{id});
      if(!snapshot.currentFile) clearNoteView();
    }
    renderShell();renderPage();
  }catch(error){status(String(error))}
}
function scheduleSearch() {
  cancelSearch();
  searchTimer = window.setTimeout(() => {
    searchTimer = undefined;
    void search();
  }, SEARCH_DELAY_MS);
}
async function search() {
  const generation = ++searchGeneration;
  const query = (document.querySelector<HTMLInputElement>("#search")?.value ?? "").trim();
  if (!query) return renderTree();
  try {
    // null means the backend abandoned this search for a newer one.
    const ids = await call<string[] | null>("search_workspace", { query });
    if (generation !== searchGeneration || !ids) return;
    const matches = new Set(ids);
    renderTree((snapshot?.entries ?? []).filter(entry => matches.has(entry.id)));
  } catch (error) {
    if (generation === searchGeneration) status(String(error));
  }
}
function rememberRenderPromise(cache: Map<string, Promise<string>>, key: string, create: () => Promise<string>) {
  const cached = cache.get(key);
  if (cached) {
    cache.delete(key);
    cache.set(key, cached);
    return cached;
  }
  const promise = create().catch(error => {
    cache.delete(key);
    throw error;
  });
  cache.set(key, promise);
  while (cache.size > MAX_RENDER_CACHE_ENTRIES) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
  return promise;
}

async function imageSource(link: string): Promise<string> {
  if (/^(data:|https?:|blob:)/i.test(link)) return link;
  const key = `${snapshot?.workspacePath ?? ""}\0${snapshot?.currentFile ?? ""}\0${link}`;
  return rememberRenderPromise(assetSourceCache, key, async () =>
    await call<string | null>("workspace_asset_data", {link}) ?? link
  );
}
async function resolveRenderedImages(html: string): Promise<string> {
  const container = document.createElement("div");
  container.innerHTML = html;
  await Promise.all(Array.from(container.querySelectorAll<HTMLImageElement>("img[src]")).map(async image => {
    image.src = await imageSource(image.getAttribute("src") ?? "");
  }));
  return container.innerHTML;
}
async function renderMarkdown(markdownSource: string): Promise<string> {
  const cacheKey = `${markdownSource}\0${footnoteLabels().join("\0")}\0${latestLinkDefinitions.join("\0")}`;
  let html = markdownHtmlCache.get(cacheKey);
  if (html === undefined) {
    html = await renderMarkdownHtml(markdownSource, footnoteLabels(), latestLinkDefinitions);
    markdownHtmlCache.set(cacheKey, html);
    while (markdownHtmlCache.size > MAX_RENDER_CACHE_ENTRIES) {
      const oldest = markdownHtmlCache.keys().next().value;
      if (oldest === undefined) break;
      markdownHtmlCache.delete(oldest);
    }
  } else {
    markdownHtmlCache.delete(cacheKey);
    markdownHtmlCache.set(cacheKey, html);
  }
  return resolveRenderedImages(html);
}
async function renderInlineMarkdown(markdownSource: string): Promise<string> {
  const html = await renderInlineMarkdownHtml(markdownSource, footnoteLabels(), latestLinkDefinitions);
  return resolveRenderedImages(html);
}
function footnoteLabels() {
  return latestBlocks
    .filter(block => blockKind(block).includes("Footnote") && block.footnoteId)
    .map(block => block.footnoteId!);
}
function clearLiveDecorations() {
  if (editor && editor.state.field(liveDecorations, false)) {
    editor.dispatch({ effects: setLiveDecorations.of(Decoration.none) });
  }
}
function byteOffsetToJsOffset(source: string, byteOffset: number) {
  if (offsetMapperSource !== source) {
    offsetMapperSource = source;
    offsetMapper = createUtf8OffsetMapper(source);
  }
  return offsetMapper(byteOffset);
}
function blockKind(block: Block) {
  return JSON.stringify(block.kind);
}
function blockMarkdownSource(block: Block) {
  if (!block.sourceRange) return block.markdown;
  const from = byteOffsetToJsOffset(currentText, block.sourceRange.start);
  const to = byteOffsetToJsOffset(currentText, block.sourceRange.end);
  return currentText.slice(from, to);
}
async function toggleLiveTask(block: Block) {
  try {
    if (typeof block.taskOffset !== "number") throw new Error("Markdown task has no source offset");
    const currentEditor = editor;
    if (!currentEditor) throw new Error("The editor is no longer available");
    const before = currentEditor.state.doc.toString();
    const source = await call<string>("toggle_markdown_task", { source: before, taskOffset: block.taskOffset });
    // Do not apply a result based on stale editor contents if the user typed
    // while the backend command was running.
    if (editor !== currentEditor || currentEditor.state.doc.toString() !== before) {
      throw new Error("The note changed while updating the task. Try again.");
    }
    // The backend toggles only the task marker. Dispatch that minimal change
    // so CodeMirror can map the selection and keep its scroll position.
    let from = 0;
    while (from < before.length && from < source.length && before[from] === source[from]) from += 1;
    let suffix = 0;
    while (suffix < before.length - from && suffix < source.length - from
      && before[before.length - suffix - 1] === source[source.length - suffix - 1]) suffix += 1;
    currentEditor.dispatch({
      changes: { from, to: before.length - suffix, insert: source.slice(from, source.length - suffix) },
      userEvent: "input.toggleTask",
    });
    await flushSave();
  } catch (error) {
    status(String(error));
  }
}
async function renderLiveBlock(block: Block, container: HTMLElement, isCurrent = () => true) {
  if (!isCurrent()) return false;
  const kind = blockKind(block);
  try {
    container.className = "live-block";
    if (kind.includes("Task")) {
      const label = document.createElement("label");
      label.className = "task";
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.checked = kind.includes("true");
      const text = document.createElement("span");
      label.append(checkbox, text);
      container.append(label);
      const html = await renderInlineMarkdown(taskLabelSource(block));
      if (!isCurrent()) return false;
      text.innerHTML = html;
      checkbox.addEventListener("change", () => void toggleLiveTask(block));
    } else if (kind.includes("Mermaid")) {
      container.className = "live-block mermaid";
      const svg = await rememberRenderPromise(mermaidCache, block.markdown, async () =>
        DOMPurify.sanitize(await call<string>("render_mermaid", { source: block.markdown }), { USE_PROFILES: { svg: true, svgFilters: true } })
      );
      if (!isCurrent()) return false;
      container.innerHTML = svg;
    } else if (kind.includes("Image") && block.image) {
      const image = document.createElement("img");
      image.alt = block.image.alt;
      image.addEventListener("error", () => {
        if (!isCurrent()) return;
        container.className = "live-block render-error";
        container.textContent = `Image unavailable: ${block.image?.alt || block.image?.destination || "unknown asset"}`;
      });
      const source = await imageSource(block.image.destination);
      if (!isCurrent()) return false;
      image.src = source;
      container.append(image);
    } else if (kind.includes("Heading")) {
      const html = await renderMarkdown(blockMarkdownSource(block));
      if (!isCurrent()) return false;
      container.innerHTML = html;
    } else if (kind.includes("Rule")) {
      container.innerHTML = "<hr>";
    } else if (kind.includes("Quote")) {
      const html = await renderMarkdown(blockMarkdownSource(block));
      if (!isCurrent()) return false;
      container.innerHTML = html;
    } else if (kind.includes("Code")) {
      const pre = document.createElement("pre");
      const code = document.createElement("code");
      code.textContent = block.markdown;
      const language = block.language?.match(/^[A-Za-z0-9_-]+/)?.[0];
      if (language) {
        code.classList.add(`language-${language}`);
        code.dataset.language = language;
      }
      pre.append(code);
      container.append(pre);
    } else if (kind.includes("Footnote")) {
      const section = document.createElement("section");
      section.className = "footnotes";
      const list = document.createElement("ol");
      const item = document.createElement("li");
      item.id = footnoteDomId(block.footnoteId ?? "");
      item.innerHTML = await renderMarkdown(footnoteBody(blockMarkdownSource(block)));
      if (!isCurrent()) return false;
      list.append(item);
      section.append(list);
      container.append(section);
    } else {
      const html = await renderMarkdown(blockMarkdownSource(block));
      if (!isCurrent()) return false;
      container.innerHTML = html;
    }
    if (!isCurrent()) return false;
    attachRenderedLinks(container);
    return true;
  } catch (error) {
    if (!isCurrent()) return false;
    container.className = "live-block render-error";
    container.textContent = "Preview error: " + (error instanceof Error ? error.message : String(error));
    return true;
  }
}
class LiveBlockWidget extends WidgetType {
  private disposed = false;
  private readonly renderKey: string;
  constructor(private readonly block: Block) {
    super();
    const kind = blockKind(block);
    const source = blockMarkdownSource(block);
    this.renderKey = JSON.stringify([
      kind,
      source,
      block.image?.alt,
      block.image?.destination,
      // Task widgets capture the source offset in their checkbox handler.
      // Recreate those when edits move the task, while keeping ordinary
      // rendered blocks stable as source ranges shift around them.
      kind.includes("Task") ? block.taskOffset : undefined,
    ]);
  }
  eq(other: WidgetType) {
    return other instanceof LiveBlockWidget && this.renderKey === other.renderKey;
  }
  toDOM() {
    const container = document.createElement("div");
    // A preview refresh does not make an unchanged live widget stale. Its
    // content key controls replacement; destruction invalidates pending work.
    const isCurrent = () => !this.disposed && editorMode === "live";
    void renderLiveBlock(this.block, container, isCurrent).catch(error => {
      if (!isCurrent()) return;
      container.className = "live-block render-error";
      container.textContent = "Preview error: " + (error instanceof Error ? error.message : String(error));
    });
    return container;
  }
  destroy() { this.disposed = true; }
  ignoreEvent(event: Event) {
    // Let clicks on rendered text reach CodeMirror so it can place the
    // cursor in the replaced source range. Interactive rendered controls
    // handle their own events and must not move the source cursor underneath.
    const target = event.target;
    return target instanceof Element && !!target.closest("a,button,input,select,textarea");
  }
}
function taskLabelSource(block: Block) {
  return blockMarkdownSource(block).replace(/^\s*(?:[-+*]|\d+[.)])\s+\[[ xX]\][ \t]*/, "");
}
function updateLiveDecorations(blocks: Block[]) {
  if (!editor || editorMode !== "live") return;
  const activeLine = editor.state.doc.lineAt(editor.state.selection.main.head);
  const ranges: { from: number; to: number; value: ReturnType<typeof Decoration.replace> }[] = [];
  let lastTo = -1;
  for (const block of blocks) {
    if (!block.sourceRange) continue;
    const from = byteOffsetToJsOffset(currentText, block.sourceRange.start);
    let to = byteOffsetToJsOffset(currentText, block.sourceRange.end);
    if (to <= from || (from < activeLine.to && to >= activeLine.from)) continue;
    if (currentText[to] === "\r") to += 1;
    if (currentText[to] === "\n") to += 1;
    if (from < lastTo || to > editor.state.doc.length) continue;
    ranges.push({ from, to, value: Decoration.replace({ widget: new LiveBlockWidget(block), block: true }) });
    lastTo = to;
  }
  editor.dispatch({ effects: setLiveDecorations.of(Decoration.set(ranges, true)) });
}
function attachRenderedLinks(root: HTMLElement) {
  root.querySelectorAll<HTMLAnchorElement>("a[href]").forEach(link => {
    // The link lives inside a CodeMirror replacement widget. Keep pointer
    // events on the anchor so the editor cannot turn them into cursor moves.
    link.addEventListener("pointerdown", event => event.stopPropagation());
    link.addEventListener("click", async event => {
      const href = link.getAttribute("href") ?? "";
      if (href.startsWith("#fn-")) {
        event.preventDefault();
        event.stopPropagation();
        const id = decodeURIComponent(href.slice(1));
        const renderedTarget = root.closest(".cm-content")?.querySelector<HTMLElement>(`#${CSS.escape(id)}`)
          ?? document.querySelector<HTMLElement>(`#preview #${CSS.escape(id)}`);
        if (renderedTarget) renderedTarget.scrollIntoView({ block: "center" });
        else {
          const definition = latestBlocks.find(block => block.footnoteId && footnoteDomId(block.footnoteId) === id);
          if (definition?.sourceRange && editor) {
            editor.dispatch({
              selection: { anchor: byteOffsetToJsOffset(currentText, definition.sourceRange.start) },
              scrollIntoView: true,
            });
          }
        }
        return;
      }
      if (!href || href.startsWith("#")) return;
      event.preventDefault();
      event.stopPropagation();
      if (/^(https?:|mailto:)/i.test(href)) {
        try { await openExternal(href); }
        catch(error) { status("Could not open link: " + error); }
        return;
      }
      if (!await saveBeforeChangingNote()) return;
      try { openNoteView(await call<Note>("navigate_markdown_link", { link: href })); }
      catch(error) { status("Open failed: " + error); }
    });
  });
}
async function refreshPreview(generation: number) {
  const preview = document.querySelector<HTMLElement>("#preview");
  try {
    const result = await call<{blocks: Block[]; linkDefinitions: string[]}>("preview_document", { source: currentText });
    if (generation !== previewGeneration) return;
    latestBlocks = result.blocks;
    latestLinkDefinitions = result.linkDefinitions ?? [];
    if (editorMode === "live") updateLiveDecorations(result.blocks);
    if (!preview) return;
    // The preview pane is not visible in Source or Live mode. Avoid building
    // a second copy of every rendered block while the user is editing.
    if (editorMode !== "split" && editorMode !== "preview") return;
    const fragment = document.createDocumentFragment();
    for (const block of result.blocks) {
      if (generation !== previewGeneration) return;
      const element = document.createElement("section");
      const rendered = await renderLiveBlock(block, element, () => generation === previewGeneration);
      if (!rendered || generation !== previewGeneration) return;
      fragment.append(element);
    }
    if (generation !== previewGeneration) return;
    const scrollTop = preview.scrollTop;
    preview.className = "";
    preview.replaceChildren(fragment);
    preview.scrollTop = scrollTop;
  } catch (error) {
    if (generation !== previewGeneration || !preview) return;
    preview.className = "preview-error";
    preview.textContent = "Preview failed: " + error;
  }
}
window.addEventListener("beforeunload",()=>void flushSave());
document.addEventListener("visibilitychange", () => {
  if (document.hidden) {
    if (iosDevice()) iosWasBackgrounded = true;
    // UIKit keeps the app alive for a bounded period during this flush. The
    // recovery draft remains the fallback if the provider is still unavailable.
    void flushSave().finally(() => {
      if (iosDevice()) void call<void>("finish_ios_background_save").catch(() => {});
    });
  } else if (iosDevice() && iosWasBackgrounded) {
    iosWasBackgrounded = false;
    void refresh();
  }
});
async function start(){
  renderShell();
  try {
    snapshot=await call<Snapshot>("workspace_snapshot");
    if (snapshot.currentFile) loadNote(await call<Note>("reload_note"));
    else { renderShell(); renderPage(); }
  } catch(error){
    // Keep an inaccessible restored note out of the editor. Never let an empty
    // buffer overwrite a note that could not be read during startup.
    snapshot=undefined;
    renderShell(); renderPage(); status(`Startup failed: ${error}`);
    return;
  }
  if (snapshot.workspaceRestoring) await finishRestore(snapshot);
}
// The backend reopens the saved favorite in the background so the window is
// usable immediately. Apply it only if the user has not changed anything since.
async function finishRestore(initial: Snapshot) {
  status("Reopening favorite workspace…");
  try {
    const restored = await call<Snapshot | null>("restored_workspace");
    if (snapshot !== initial) return;
    if (!restored) { status("Could not reopen the favorite workspace"); return; }
    snapshot = restored;
    if (!restored.currentFile) { renderShell(); renderPage(); status("Ready"); return; }
    try {
      const note = await call<Note>("reload_note");
      if (snapshot === restored) loadNote(note);
    } catch (error) {
      if (snapshot !== restored) return;
      // As at startup, never leave an unreadable note open for editing.
      snapshot = { ...restored, currentFile: undefined };
      renderShell(); renderPage(); status(`Could not open the last note: ${error}`);
    }
  } catch (error) {
    if (snapshot === initial) status(`Could not reopen the favorite workspace: ${error}`);
  }
}
void start();
