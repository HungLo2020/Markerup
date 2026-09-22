import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import { EditorState, StateEffect, StateField } from "@codemirror/state";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { Decoration, drawSelection, keymap, EditorView, WidgetType, type DecorationSet } from "@codemirror/view";
import DOMPurify from "dompurify";
import { marked } from "marked";
import "./styles.css";

const menuIcon = new URL("../../resources/icon_menu.svg", import.meta.url).href;
const settingsIcon = new URL("../../resources/icon_preview_settings.svg", import.meta.url).href;
const folderIcon = new URL("../../resources/icon_folder.svg", import.meta.url).href;

type Entry = { id: string; name: string; kind: "File" | "Directory"; depth: number };
type Favorite = { index: number; label: string; workspaceIsSmb: boolean };
type Snapshot = { workspaceOpen: boolean; workspacePath: string; workspaceIsSmb: boolean; workspaceFavorited: boolean; favorites: Favorite[]; entries: Entry[]; currentFile?: string; canGoBack: boolean; canGoForward: boolean; externalConflict: boolean };
type Note = { id: string; contents: string; snapshot: Snapshot };
type SourceRange = { start: number; end: number };
type Block = { kind: unknown; markdown: string; taskOffset?: number; sourceRange?: SourceRange; image?: { alt: string; destination: string } };

let snapshot: Snapshot | undefined;
let currentText = "";
let savedText = "";
let saveTimer: number | undefined;
let saveInFlight: Promise<void> | undefined;
let retryTimer: number | undefined;
let saveBlockedUntilReload = false;
let editor: EditorView;
let page: "main" | "settings" | "location" | "smb" | "about" = "main";
let editorMode: "source" | "live" | "split" | "preview" = "split";
const collapsedDirectories = new Set<string>();
let previewGeneration = 0;
let latestBlocks: Block[] = [];

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
const call = <T>(command: string, args?: Record<string, unknown>) => invoke<T>(command, args);
const mobileLayout = () => window.matchMedia("(max-width: 700px)").matches;
// iPadOS can present a desktop-style user agent, so include its touch-capable
// MacIntel form as well as the conventional iOS device identifiers. This must
// be a platform check rather than a viewport check: desktop mobile-preview
// windows still use Tauri's desktop dialog plugin.
const iosDevice = () => /iPad|iPhone|iPod/.test(navigator.userAgent)
  || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);

function renderShell() {
  app.innerHTML = `<header><button id="menu" class="icon-button" aria-label="Toggle workspace"><img src="${menuIcon}" alt=""></button><strong>Markerup</strong><span id="location">${escape(snapshot?.workspacePath ?? "No workspace")}</span><span class="grow"></span><button id="back">←</button><button id="forward">→</button><button id="refresh">Refresh</button><button id="settings" class="icon-button" aria-label="Settings"><img src="${settingsIcon}" alt=""></button></header><main id="content"></main><footer id="status">Ready</footer>`;
  document.querySelector("#menu")!.addEventListener("click", () => document.body.classList.toggle("sidebar-hidden"));
  document.querySelector("#settings")!.addEventListener("click", () => { page = "settings"; renderPage(); });
  document.querySelector("#refresh")!.addEventListener("click", refresh);
  document.querySelector("#back")!.addEventListener("click", () => navigate("go_back"));
  document.querySelector("#forward")!.addEventListener("click", () => navigate("go_forward"));
}

function renderPage() {
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
  content.innerHTML = `<aside id="sidebar"><div class="row"><strong>Workspace</strong><button id="new" aria-label="Create">＋</button></div><input id="search" placeholder="Search all notes"><nav id="tree"></nav></aside><section id="document"><div class="document-bar"><strong>${escape(snapshot?.currentFile ?? "Choose a note")}</strong><span class="grow"></span>${snapshot?.currentFile ? `<button id="insert">Insert</button>` : ""}${viewControls}</div><div id="panes"><div id="editor-pane"><div id="editor"></div></div><article id="preview"></article></div></section>`;
  document.querySelector("#new")!.addEventListener("click",()=>createAtRoot());
  document.querySelector("#search")!.addEventListener("input", search);
  document.querySelector("#insert")?.addEventListener("click", () => void showInsertMenu());
  document.querySelector<HTMLSelectElement>("#view-mode")!.addEventListener("change", event => {
    editorMode = (event.target as HTMLSelectElement).value as typeof editorMode;
    applyMode();
  });
  renderTree(); setupEditor(); void refreshPreview(); applyMode();
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
function setupEditor() {
  const host=document.querySelector<HTMLElement>("#editor")!;
  editor = new EditorView({ state: EditorState.create({ doc: currentText, extensions: [liveDecorations, history(), markdown(), keymap.of([...defaultKeymap,...historyKeymap]), EditorView.lineWrapping, drawSelection({iosSelectionHandles:true}), EditorView.theme({"&":{height:"100%"},".cm-scroller":{overflow:"auto",fontFamily:"inherit",lineHeight:"1.28"},".cm-content":{lineHeight:"1.28",padding:"12px"},".cm-line":{lineHeight:"1.28"},".cm-selectionBackground":{backgroundColor:"rgba(10, 132, 255, 0.30)"},"&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground":{backgroundColor:"rgba(10, 132, 255, 0.52)"}}, {dark:true}), EditorView.updateListener.of(update=>{if(update.docChanged){currentText=update.state.doc.toString();clearLiveDecorations();scheduleSave();void refreshPreview();} else if(update.selectionSet && editorMode === "live"){updateLiveDecorations(latestBlocks);}})] }), parent:host });
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
function loadNote(note:Note){ if(saveTimer) clearTimeout(saveTimer); if(retryTimer) clearTimeout(retryTimer); saveBlockedUntilReload=false; snapshot=note.snapshot; currentText=savedText=note.contents; renderShell(); renderPage(); status("Saved"); }
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
async function refresh(){ await flushSave(); try { snapshot=await call<Snapshot>("refresh_workspace",{editorHasUnsavedChanges:currentText!==savedText}); renderShell(); renderPage(); status("Workspace refreshed"); } catch(error){status(`Refresh failed: ${error}`)} }
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
    page="main";
    renderShell();
    renderPage();
    status("Local workspace selected");
  } catch(error) {
    status(`Workspace selection failed: ${error}`);
  }
}
async function connectSmb(){ if(!await saveBeforeChangingNote()) return; const value=(id:string) => document.querySelector<HTMLInputElement>(`#${id}`)!.value; try { snapshot=await call<Snapshot>("connect_smb",{request:{server:value("server"),share:value("share"),username:value("username"),password:value("password"),remotePath:value("remote")}}); page="main";renderShell();renderPage();status("SMB workspace connected"); } catch(error){status(`SMB connection failed: ${error}`)} }
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
  const selection = editor.state.selection.main;
  editor.dispatch({
    changes: { from: selection.from, to: selection.to, insert: text },
    selection: { anchor: selection.from + text.length },
    userEvent: "input.insert",
  });
  editor.focus();
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
    if(action==="delete" && await confirmAction("Delete entry", `Delete ${id.split("/").pop() ?? "this entry"}?`)) snapshot=await call<Snapshot>("delete_entry",{id});
    renderShell();renderPage();
  }catch(error){status(String(error))}
}
async function search(){ const query=(document.querySelector<HTMLInputElement>("#search")?.value ?? "").trim(); if(!query)return renderTree(); try {const ids=await call<string[]>("search_workspace",{query});renderTree((snapshot?.entries??[]).filter(e=>ids.includes(e.id)));}catch(error){status(String(error))} }
async function imageSource(link: string): Promise<string> {
  if (/^(data:|https?:|blob:)/i.test(link)) return link;
  return await call<string | null>("workspace_asset_data", {link}) ?? link;
}
async function renderMarkdown(markdownSource: string): Promise<string> {
  const html = DOMPurify.sanitize(await marked.parse(markdownSource));
  const container = document.createElement("div");
  container.innerHTML = html;
  await Promise.all(Array.from(container.querySelectorAll<HTMLImageElement>("img[src]")).map(async image => {
    image.src = await imageSource(image.getAttribute("src") ?? "");
  }));
  return container.innerHTML;
}
function clearLiveDecorations() {
  if (editor && editor.state.field(liveDecorations, false)) {
    editor.dispatch({ effects: setLiveDecorations.of(Decoration.none) });
  }
}
function byteOffsetToJsOffset(source: string, byteOffset: number) {
  const encoder = new TextEncoder();
  let bytes = 0;
  let offset = 0;
  for (const character of source) {
    if (bytes >= byteOffset) break;
    bytes += encoder.encode(character).length;
    offset += character.length;
  }
  return offset;
}
function blockKind(block: Block) {
  return JSON.stringify(block.kind);
}
async function toggleLiveTask(block: Block) {
  try {
    if (typeof block.taskOffset !== "number") throw new Error("Markdown task has no source offset");
    const source = await call<string>("toggle_markdown_task", { source: currentText, taskOffset: block.taskOffset });
    editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: source }, userEvent: "input.toggleTask" });
    await flushSave();
  } catch (error) {
    status(String(error));
  }
}
async function renderLiveBlock(block: Block, container: HTMLElement) {
  const kind = blockKind(block);
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
    text.innerHTML = await renderMarkdown(block.markdown);
    checkbox.addEventListener("change", () => void toggleLiveTask(block));
  } else if (kind.includes("Mermaid")) {
    container.className = "live-block mermaid";
    try {
      container.innerHTML = DOMPurify.sanitize(await call<string>("render_mermaid", { source: block.markdown }), { USE_PROFILES: { svg: true, svgFilters: true } });
    } catch (error) {
      container.className = "live-block mermaid-error";
      container.textContent = "Mermaid error: " + error;
    }
  } else if (kind.includes("Image") && block.image) {
    const image = document.createElement("img");
    image.alt = block.image.alt;
    image.src = await imageSource(block.image.destination);
    container.append(image);
  } else if (kind.includes("Heading")) {
    const match = kind.match(/Heading[^0-9]*(\d+)/);
    const heading = document.createElement("h" + Math.min(6, Math.max(1, Number(match?.[1] ?? 1))));
    heading.innerHTML = await renderMarkdown(block.markdown);
    container.append(heading);
  } else if (kind.includes("Rule")) {
    container.innerHTML = "<hr>";
  } else if (kind.includes("Quote")) {
    const quote = document.createElement("blockquote");
    quote.innerHTML = await renderMarkdown(block.markdown);
    container.append(quote);
  } else if (kind.includes("Code")) {
    const pre = document.createElement("pre");
    const code = document.createElement("code");
    code.textContent = block.markdown;
    pre.append(code);
    container.append(pre);
  } else {
    container.innerHTML = await renderMarkdown(block.markdown);
  }
  attachRenderedLinks(container);
}
class LiveBlockWidget extends WidgetType {
  constructor(private readonly block: Block) { super(); }
  toDOM() {
    const container = document.createElement("div");
    void renderLiveBlock(this.block, container);
    return container;
  }
  ignoreEvent() { return true; }
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
async function refreshPreview() {
  const preview = document.querySelector<HTMLElement>("#preview");
  const generation = ++previewGeneration;
  const result = await call<{blocks: Block[]}>("preview_document", { source: currentText });
  if (generation !== previewGeneration) return;
  latestBlocks = result.blocks;
  if (editorMode === "live") updateLiveDecorations(result.blocks);
  if (!preview) return;
  preview.innerHTML = "";
  for (const block of result.blocks) {
    const element = document.createElement("section");
    await renderLiveBlock(block, element);
    preview.append(element);
  }
  attachRenderedLinks(preview);
}
window.addEventListener("beforeunload",()=>void flushSave()); document.addEventListener("visibilitychange",()=>{if(document.hidden)void flushSave()});
async function start(){ renderShell(); try{snapshot=await call<Snapshot>("workspace_snapshot");renderShell();renderPage();}catch(error){status(`Startup failed: ${error}`)} }
void start();
