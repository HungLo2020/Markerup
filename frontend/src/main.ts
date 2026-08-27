import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import { EditorState } from "@codemirror/state";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { drawSelection, keymap, EditorView } from "@codemirror/view";
import DOMPurify from "dompurify";
import { marked } from "marked";
import "./styles.css";

const menuIcon = new URL("../../resources/icon_menu.svg", import.meta.url).href;
const settingsIcon = new URL("../../resources/icon_preview_settings.svg", import.meta.url).href;
const folderIcon = new URL("../../resources/icon_folder.svg", import.meta.url).href;

type Entry = { id: string; name: string; kind: "File" | "Directory"; depth: number };
type Snapshot = { workspaceOpen: boolean; workspacePath: string; workspaceIsSmb: boolean; workspacePinned: boolean; entries: Entry[]; currentFile?: string; canGoBack: boolean; canGoForward: boolean; externalConflict: boolean };
type Note = { id: string; contents: string; snapshot: Snapshot };
type Block = { kind: unknown; markdown: string; taskOffset?: number; image?: { alt: string; destination: string } };

let snapshot: Snapshot | undefined;
let currentText = "";
let savedText = "";
let saveTimer: number | undefined;
let editor: EditorView;
let page: "main" | "settings" | "location" | "smb" | "about" = "main";
let editorMode: "source" | "split" | "preview" = "split";

const app = document.querySelector<HTMLDivElement>("#app")!;
const status = (message: string) => document.querySelector<HTMLElement>("#status")!.textContent = message;
const escape = (value: string) => value.replace(/[&<>"']/g, char => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[char]!));
const call = <T>(command: string, args?: Record<string, unknown>) => invoke<T>(command, args);
const mobileLayout = () => window.matchMedia("(max-width: 700px)").matches;

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
  if (page === "location") { content.innerHTML = panel("Location", `<p>${snapshot?.workspaceOpen ? `${snapshot.workspaceIsSmb ? "SMB" : "Local"} workspace: ${escape(snapshot.workspacePath)}` : "No workspace selected."}</p><button id="browse">Browse local folders</button><button id="smb">Connect to SMB</button><button id="pin">${snapshot?.workspacePinned ? "Unpin workspace" : "Pin workspace"}</button>`); document.querySelector("#browse")!.addEventListener("click", chooseLocal); document.querySelector("#smb")!.addEventListener("click",()=>{page="smb";renderPage()}); document.querySelector("#pin")!.addEventListener("click",togglePin); return; }
  if (page === "smb") { content.innerHTML = panel("Connect to SMB", `<label>Server<input id="server" placeholder="server or IP"></label><label>Share<input id="share"></label><label>Username<input id="username"></label><label>Password<input id="password" type="password"></label><label>Remote folder<input id="remote" placeholder="Notes"></label><button id="connect">Connect</button>`); document.querySelector("#connect")!.addEventListener("click",connectSmb); return; }
  if (page === "about") { content.innerHTML = panel("About Markerup", `<p>Version 0.4.0</p><button id="privacy">Privacy Policy</button>`); document.querySelector("#privacy")!.addEventListener("click",async()=>openExternal(await call<string>("privacy_policy_url"))); return; }
  const viewControls = mobileLayout()
    ? `<button id="mobile-view-toggle">${editorMode === "source" ? "Preview" : "Source"}</button>`
    : `<button data-mode="source">Source</button><button data-mode="split">Split</button><button data-mode="preview">Preview</button>`;
  content.innerHTML = `<aside id="sidebar"><div class="row"><strong>Workspace</strong><button id="new" aria-label="Create">＋</button></div><input id="search" placeholder="Search all notes"><nav id="tree"></nav></aside><section id="document"><div class="document-bar"><strong>${escape(snapshot?.currentFile ?? "Choose a note")}</strong><span class="grow"></span>${viewControls}</div><div id="panes"><div id="editor-pane"><div id="editor"></div></div><article id="preview"></article></div></section>`;
  document.querySelector("#new")!.addEventListener("click",()=>createAtRoot());
  document.querySelector("#search")!.addEventListener("input", search);
  document.querySelectorAll<HTMLButtonElement>("[data-mode]").forEach(button => button.addEventListener("click",()=>{editorMode=button.dataset.mode as typeof editorMode; applyMode()}));
  document.querySelector("#mobile-view-toggle")?.addEventListener("click", () => {
    editorMode = editorMode === "source" ? "preview" : "source";
    renderPage();
  });
  renderTree(); setupEditor(); renderPreview(); applyMode();
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

async function confirmAction(title: string, message: string): Promise<boolean> {
  const answer = await chooseAction(title, [{ id: "cancel", label: "Cancel" }, { id: "confirm", label: message, destructive: true }]);
  return answer === "confirm";
}

function renderTree(entries = snapshot?.entries ?? []) {
  const tree=document.querySelector("#tree"); if (!tree) return;
  tree.innerHTML=entries.map(entry=>`<div class="entry" style="padding-left:${entry.depth * 16 + 6}px"><button class="entry-main" data-id="${escape(entry.id)}">${entry.kind === "Directory" ? `<img class="entry-folder-icon" src="${folderIcon}" alt="">` : "·"} ${escape(entry.name)}</button><button class="entry-actions" data-id="${escape(entry.id)}" data-kind="${entry.kind}">…</button></div>`).join("") || "<p class=muted>No Markdown notes found.</p>";
  tree.querySelectorAll<HTMLButtonElement>(".entry-main").forEach(b=>b.addEventListener("click",()=>openNote(b.dataset.id!)));
  tree.querySelectorAll<HTMLButtonElement>(".entry-actions").forEach(b=>b.addEventListener("click",()=>entryActions(b.dataset.id!, b.dataset.kind === "Directory")));
}
function setupEditor() {
  const host=document.querySelector<HTMLElement>("#editor")!;
  editor = new EditorView({ state: EditorState.create({ doc: currentText, extensions: [history(), markdown(), keymap.of([...defaultKeymap,...historyKeymap]), EditorView.lineWrapping, drawSelection({iosSelectionHandles:true}), EditorView.theme({"&":{height:"100%"},".cm-scroller":{overflow:"auto",fontFamily:"inherit",lineHeight:"1.28"},".cm-content":{lineHeight:"1.28",padding:"12px"},".cm-line":{lineHeight:"1.28"},".cm-selectionBackground":{backgroundColor:"rgba(10, 132, 255, 0.30)"},"&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground":{backgroundColor:"rgba(10, 132, 255, 0.52)"}}, {dark:true}), EditorView.updateListener.of(update=>{if(update.docChanged){currentText=update.state.doc.toString();scheduleSave();renderPreview()}})] }), parent:host });
}
function applyMode(){ const panes=document.querySelector("#panes"); if(panes) panes.className=editorMode; }
async function openNote(id:string){
  await flushSave();
  try {
    const note=await call<Note>("open_note",{id});
    openNoteView(note);
  } catch(error){ status(`Open failed: ${error}`); }
}
function openNoteView(note: Note) {
  if (mobileLayout()) {
    editorMode="preview";
    document.body.classList.add("sidebar-hidden");
  }
  loadNote(note);
}
function loadNote(note:Note){ snapshot=note.snapshot; currentText=savedText=note.contents; renderShell(); renderPage(); status("Saved"); }
function scheduleSave(){ status("Unsaved changes"); if(saveTimer) clearTimeout(saveTimer); saveTimer=window.setTimeout(()=>void flushSave(),750); }
async function flushSave(){ if(!snapshot?.currentFile || currentText===savedText) return; status("Saving…"); try { snapshot=await call<Snapshot>("save_note",{contents:currentText,force:false}); savedText=currentText; status("Saved"); } catch(error){ status(`Save failed — retrying: ${error}`); } }
async function refresh(){ await flushSave(); try { snapshot=await call<Snapshot>("refresh_workspace",{editorHasUnsavedChanges:currentText!==savedText}); renderShell(); renderPage(); status("Workspace refreshed"); } catch(error){status(`Refresh failed: ${error}`)} }
async function navigate(command:string){ await flushSave(); const note=await call<Note|null>(command); if(note) loadNote(note); }
async function chooseLocal(){ try { const selected=await openDialog({directory:true,multiple:false}); if(typeof selected === "string") { snapshot=await call<Snapshot>("open_local_workspace",{path:selected}); page="main";renderShell();renderPage(); } } catch(error){ status(`Workspace selection failed: ${error}`); } }
async function connectSmb(){ const value=(id:string) => document.querySelector<HTMLInputElement>(`#${id}`)!.value; try { snapshot=await call<Snapshot>("connect_smb",{request:{server:value("server"),share:value("share"),username:value("username"),password:value("password"),remotePath:value("remote")}}); page="main";renderShell();renderPage();status("SMB workspace connected"); } catch(error){status(`SMB connection failed: ${error}`)} }
async function togglePin(){ try { snapshot=await call<Snapshot>("set_workspace_pinned",{pinned:!snapshot?.workspacePinned}); renderShell();renderPage(); }catch(error){status(String(error))} }
async function createAtRoot(){
  const type = await chooseAction("Create in workspace", [{ id: "note", label: "New note" }, { id: "folder", label: "New folder" }]);
  if(type === "note") return createEntry("",true);
  if(type === "folder") return createEntry("",false);
}
async function createEntry(parent:string,note:boolean){ const name=await requestName(note?"New note":"New folder"); if(!name)return; try { if(note){loadNote(await call<Note>("create_note",{parent,name}));} else {snapshot=await call<Snapshot>("create_folder",{parent,name});renderShell();renderPage();} }catch(error){status(String(error))} }
async function entryActions(id:string,isDirectory:boolean){
  const actions: ModalAction[] = [];
  if (isDirectory) actions.push({ id: "new-note", label: "New note" }, { id: "new-folder", label: "New folder" });
  actions.push({ id: "rename", label: "Rename" }, { id: "delete", label: "Delete", destructive: true });
  const action = await chooseAction(id.split("/").pop() ?? "Actions", actions);
  if(!action)return;
  if(action==="new-note")return createEntry(id,true);
  if(action==="new-folder")return createEntry(id,false);
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
async function renderPreview(){ const preview=document.querySelector<HTMLElement>("#preview"); if(!preview)return; const {blocks}=await call<{blocks:Block[]}>("preview_document",{source:currentText}); preview.innerHTML=""; for(const block of blocks){const element=document.createElement("section"); const kind=JSON.stringify(block.kind); if(kind.includes("Task")){const checked=kind.includes("true");element.innerHTML=`<label class="task"><input type="checkbox" ${checked?"checked":""}>${DOMPurify.sanitize(await marked.parse(block.markdown))}</label>`; element.querySelector("input")!.addEventListener("change",async()=>{try{if(typeof block.taskOffset !== "number") throw new Error("Markdown task has no source offset");const source=await call<string>("toggle_markdown_task",{source:currentText,taskOffset:block.taskOffset});currentText=source;editor.dispatch({changes:{from:0,to:editor.state.doc.length,insert:source}});await flushSave();}catch(error){status(String(error))}});} else if(kind.includes("Mermaid")){element.className="mermaid";try{element.innerHTML=DOMPurify.sanitize(await call<string>("render_mermaid",{source:block.markdown}),{USE_PROFILES:{svg:true,svgFilters:true}});}catch(error){element.className="mermaid-error";element.textContent=`Mermaid error: ${error}`;}} else {element.innerHTML=DOMPurify.sanitize(await marked.parse(block.markdown));} preview.append(element); }
  preview.querySelectorAll<HTMLAnchorElement>("a[href]").forEach(link => link.addEventListener("click", async event => {
    const href=link.getAttribute("href") ?? "";
    if (!href || href.startsWith("#")) return;
    event.preventDefault();
    if (/^(https?:|mailto:)/i.test(href)) {
      try { await openExternal(href); }
      catch(error) { status(`Could not open link: ${error}`); }
      return;
    }
    await flushSave();
    try { openNoteView(await call<Note>("navigate_markdown_link", {link: href})); }
    catch(error) { status(`Open failed: ${error}`); }
  }));
}
window.addEventListener("beforeunload",()=>void flushSave()); document.addEventListener("visibilitychange",()=>{if(document.hidden)void flushSave()});
async function start(){ renderShell(); try{snapshot=await call<Snapshot>("workspace_snapshot");renderShell();renderPage();}catch(error){status(`Startup failed: ${error}`)} }
void start();
