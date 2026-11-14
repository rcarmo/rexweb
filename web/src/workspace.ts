import { WebTerminal } from "./webterm/terminal";
import { bindWorkspaceViewport } from "./viewport";
import { previewSplit, type Layout, type Rect } from "./layout";

bindWorkspaceViewport();

interface Block {
  block_id: string;
  label: string;
}

interface Session {
  session_id: string;
  label: string;
}

interface WorkspaceWindow {
  window_id: string;
  label: string;
  focused_block_id?: string;
  layers: { kind: string; layout: Layout | null; blocks: Block[] | null }[] | null;
}

interface TerminalMetadata { title:string; pwd:string; revision:number; state:string; app?:string; message?:string; status_title?:string; progress?:number; }
interface View extends Session {
  terminal_metadata?:Record<string,TerminalMetadata>;
  revision: number;
  active_window_id?: string;
  windows: WorkspaceWindow[] | null;
}

const sessionLabel = document.querySelector<HTMLElement>("#session-label")!;
const windows = document.querySelector<HTMLElement>("#windows")!;
const workspace = document.querySelector<HTMLElement>("#workspace")!;
const status = document.querySelector<HTMLElement>("#status")!;
const chooser = document.querySelector<HTMLDialogElement>("#chooser")!;
const filter = document.querySelector<HTMLInputElement>("#chooser-filter")!;
const chooserItems = document.querySelector<HTMLElement>("#chooser-items")!;
const actionDialog = document.querySelector<HTMLDialogElement>("#action-dialog")!;
const actionName = document.querySelector<HTMLInputElement>("#action-name")!;
const appearance = matchMedia("(prefers-color-scheme: dark)");

const panes = new Map<string, { element: HTMLElement; terminal?: WebTerminal }>();
const rendererChoice = new URLSearchParams(location.search).get('renderer') === 'webgpu' ? 'webgpu' : 'canvas';
const bidiEnabled = new URLSearchParams(location.search).get('bidi') === '1';
function chooseBidi() {
  const url = new URL(location.href);
  if (bidiEnabled) url.searchParams.delete('bidi'); else url.searchParams.set('bidi', '1');
  if (sessionId) url.searchParams.set('session', sessionId);
  location.assign(url.href);
}
function chooseRenderer(renderer: 'canvas' | 'webgpu') {
  const url = new URL(location.href);
  if (renderer === 'canvas') url.searchParams.delete('renderer'); else url.searchParams.set('renderer', renderer);
  if (sessionId) url.searchParams.set('session', sessionId);
  location.assign(url.href);
}
let sessionId = new URLSearchParams(location.search).get("session") ?? "";
let view: View | undefined;
let activeWindow: WorkspaceWindow | undefined;
let loading = false;
let acting = false;
let dragging = false;
let sessionList: Session[] = [];
let chooserMode: "sessions" | "commands" = "sessions";
let pendingAction: (() => void) | undefined;

const searchBar = document.querySelector<HTMLElement>("#search-bar")!;
const searchText = document.querySelector<HTMLInputElement>("#search-text")!;
const searchResults =
  document.querySelector<HTMLOutputElement>("#search-results")!;
const searchCase=document.querySelector<HTMLInputElement>('#search-case')!;
const searchWord=document.querySelector<HTMLInputElement>('#search-word')!;
const searchRegex=document.querySelector<HTMLInputElement>('#search-regex')!;
let searchTerminal: WebTerminal | undefined;
let searchTimer: number | undefined;

function runSearch(older?: boolean) {
  if (searchBar.hidden) return;
  const target = panes.get(activeWindow?.focused_block_id ?? "")?.terminal;
  if (target !== searchTerminal) {
    searchTerminal?.clearSearch();
    searchTerminal = target;
  }
  if (!target) {
    searchResults.textContent = "No active pane";
    return;
  }
  target.search(searchText.value, older, (result) => {
    if (searchBar.hidden || target !== searchTerminal) return;
    searchResults.textContent=result.error?'Invalid expression':result.pending?'Searching…':result.resultCount?`${result.resultIndex+1} / ${result.resultCount}${result.truncated?'+':''}`:'0 matches';
    searchResults.title=result.error??(result.truncated?'Search limit reached; results are partial.':'');
  },{regex:searchRegex.checked,caseSensitive:searchCase.checked,wholeWord:searchWord.checked});
}
for(const option of [searchCase,searchWord,searchRegex])option.addEventListener('change',()=>runSearch());

function openSearch() {
  if (chooser.open || actionDialog.open) return;
  searchBar.hidden = false;
  runSearch();
  searchText.focus();
  searchText.select();
}
function closeSearch() {
  searchBar.hidden = true;
  if (searchTimer !== undefined) clearTimeout(searchTimer);
  searchTimer = undefined;
  searchTerminal?.clearSearch();
  searchTerminal = undefined;
  focusTerminal();
}
document.querySelector("#find-pane")!.addEventListener("click", openSearch);
document.querySelector("#search-close")!.addEventListener("click", closeSearch);
document
  .querySelector("#search-older")!
  .addEventListener("click", () => runSearch(true));
document
  .querySelector("#search-newer")!
  .addEventListener("click", () => runSearch(false));
searchText.addEventListener("input", () => {
  if (searchTimer !== undefined) clearTimeout(searchTimer);
  searchTimer = window.setTimeout(() => {
    searchTimer = undefined;
    runSearch();
  }, 80);
});
searchText.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    runSearch(!event.shiftKey);
  }
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    closeSearch();
  }
});

async function exportFocused(format: 'text'|'html'|'vt') {
  const block=activeWindow?.focused_block_id;
  if(!block)return;
  try {
    const query=new URLSearchParams({session:sessionId,block,format,trim:'true'});
    const response=await fetch('/api/capture?'+query);
    if(!response.ok){const error=await response.json();throw new Error(error.message??'Capture failed');}
    const url=URL.createObjectURL(await response.blob());
    const link=document.createElement('a');link.href=url;link.download=`terminal.${format==='text'?'txt':format}`;link.click();
    setTimeout(()=>URL.revokeObjectURL(url),30000);
  }catch(error){status.textContent=String(error);}
}

let uploadsEnabled=false;
const uploadInput=document.querySelector<HTMLInputElement>('#upload-file')!;
void fetch('/api/capabilities').then(response=>response.json()).then(value=>{uploadsEnabled=!!value.uploads;});
async function uploadToPane(file:File,session:string,block:string):Promise<string> {
  if(!uploadsEnabled)throw new Error('Uploads are disabled; configure -uploads with web password authentication.');
  const body=new FormData();body.append('file',file,file.name);
  const response=await fetch('/api/upload?'+new URLSearchParams({session,block}),{method:'POST',body});
  const result=await response.json();if(!response.ok)throw new Error(result.message??'Upload failed');
  status.textContent=`Uploaded ${result.display_name||result.name}`;return result.path;
}
uploadInput.addEventListener('change',()=>{
  const terminal=panes.get(activeWindow?.focused_block_id??'')?.terminal;
  const files=Array.from(uploadInput.files??[]);uploadInput.value='';
  if(files.length>8){status.textContent='Select at most eight files.';return;}
  if(terminal)void (async()=>{for(const file of files)await terminal.uploadFile(file)})().catch(error=>{status.textContent=String(error);});
});
workspace.addEventListener('upload-error',event=>{status.textContent=(event as CustomEvent).detail;});

function icon(name: string) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.classList.add("icon");
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS(svg.namespaceURI, "use");
  use.setAttribute("href", `#icon-${name}`);
  svg.append(use);
  return svg;
}

function terminalTheme() {
  return {
    ...WebTerminal.getTheme("tango")!,
    background: appearance.matches ? "#28282c" : "#f4f4f6",
    foreground: appearance.matches ? "#e2e2e7" : "#424449",
    cursor: appearance.matches ? "#b4aaff" : "#7568ef",
    cursorAccent: appearance.matches ? "#28282c" : "#f4f4f6",
    selectionBackground: appearance.matches ? "#514775" : "#dcd7fa",
    selectionForeground: appearance.matches ? "#ffffff" : "#302d40",
  };
}

function focusTerminal() {
  if (!chooser.open && !actionDialog.open && searchBar.hidden) {
    panes.get(activeWindow?.focused_block_id ?? "")?.terminal?.focus();
  }
}

async function api<T>(path: string, body?: Record<string, unknown>): Promise<T> {
  const response = await fetch(path, body && {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    const error = await response.json().catch(() => ({}));
    throw new Error(error.message ?? `Request failed (${response.status})`);
  }
  return response.json();
}

function updateMetadata(next:View) {
  for(const [id,metadata] of Object.entries(next.terminal_metadata??{})){
    const pane=panes.get(id);if(!pane)continue;
    const label=pane.element.querySelector<HTMLElement>('.pane-label');
    if(label){label.textContent=metadata.title||'Shell';label.title=metadata.pwd||'';}
    const badge=pane.element.querySelector<HTMLElement>('.pane-status');
    if(badge){badge.hidden=!metadata.state||metadata.state==='idle';badge.dataset.state=metadata.state;badge.textContent=metadata.state+(metadata.progress!==undefined?` ${metadata.progress}%`:'');badge.title=metadata.message||metadata.status_title||metadata.app||metadata.state;}
  }
}

function selectSession(id: string) {
  if (id === sessionId) return;
  sessionId = id;
  view = undefined;
  render();
  const url = new URL(location.href);
  if (id) url.searchParams.set("session", id);
  else url.searchParams.delete("session");
  history.replaceState(null, "", url);
}

async function refresh() {
  if (loading || acting || dragging) return;
  loading = true;
  try {
    const { sessions: available } = await api<{ sessions: Session[] | null }>("/api/sessions");
    const list = available ?? [];
    if (!list.some((entry) => entry.session_id === sessionId)) {
      selectSession(list[0]?.session_id ?? "");
    }
    sessionList = list;
    sessionLabel.textContent = list.find((entry) => entry.session_id === sessionId)?.label || "Sessions";
    const id = sessionId;
    if (id) {
      const next = await api<View>(`/api/session?${new URLSearchParams({ session: id })}`);
      if (!dragging && id === sessionId && (!view || next.revision > view.revision)) {
        view = next;
        render();
      }
      updateMetadata(next);
    }
    status.textContent = id ? "" : "Create or select a session.";
  } catch (error) {
    status.textContent = String(error);
  } finally {
    loading = false;
  }
}

async function act(op: string, args: Record<string, unknown> = {}) {
  if (acting) return;
  acting = true;
  try {
    const result = await api<{ session_id?: string }>("/api/action", { op, session: sessionId, ...args });
    if (op === "create" && result.session_id) selectSession(result.session_id);
  } catch (error) {
    status.textContent = String(error);
    return;
  } finally {
    acting = false;
  }
  await refresh();
  focusTerminal();
}

function place(element: HTMLElement, [x, y, width, height]: Rect) {
  Object.assign(element.style, {
    left: `${100 * x}%`, top: `${100 * y}%`,
    width: `${100 * width}%`, height: `${100 * height}%`,
  });
}

function mountPane(block: Block, rect: Rect, keep: Set<string>) {
  const id = block.block_id;
  keep.add(id);
  let pane = panes.get(id);
  if (!pane) {
    const element = document.createElement("section");
    element.className = "pane";
    const header = document.createElement("header");
    header.className = "pane-header";
    const title = document.createElement("div");
    title.className = "pane-title";
    const label = document.createElement("span");
    label.className = "pane-label";
    const badge=document.createElement('span');badge.className='pane-status';badge.hidden=true;
    title.append(icon("terminal"), label, badge);
    const actions = document.createElement("div");
    actions.className = "pane-actions";
    for (const [op, name, glyph] of [
      ["split-right", "Split right", "right"],
      ["split-down", "Split down", "down"],
      ["close-block", "Close pane", "close"],
    ]) {
      const button = document.createElement("button");
      button.className = "icon-button";
      button.title = name;
      button.setAttribute("aria-label", name);
      button.append(icon(glyph));
      button.addEventListener("click", () => runCommand(op, id));
      actions.append(button);
    }
    header.append(title, actions);
    const terminal = document.createElement("div");
    terminal.className = "webterm-terminal";
    const viewport = document.createElement("div");
    viewport.className = "terminal-viewport";
    viewport.append(terminal);
    element.append(header, viewport);
    workspace.append(element);
    pane = { element };
    panes.set(id, pane);
    element.addEventListener("pointerdown", (event) => {
      if ((event.target as Element).closest("button")) return;
      if (event.pointerType === "mouse") panes.get(id)?.terminal?.focus(true);
      if (activeWindow?.focused_block_id !== id) void act("focus-block", { block: id });
    });
    const url = new URL("/api/terminal", location.href);
    url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
    url.search = new URLSearchParams({ session: sessionId, block: id }).toString();
    const mounted = pane;
    const ownerSession=sessionId;
    void WebTerminal.create(terminal, url.href, { autofocus: false, fontSize: 14, renderer: rendererChoice, bidi: bidiEnabled, theme: terminalTheme(), uploadFile:file=>uploadToPane(file,ownerSession,id) }).then((instance) => {
      if (panes.get(id) === mounted) {
        mounted.terminal = instance;
        if (activeWindow?.focused_block_id === id) { focusTerminal(); runSearch(); }
      }
      else instance.dispose();
    }).catch((error) => { terminal.textContent = String(error); });
  }
  pane.element.querySelector(".pane-label")!.textContent = block.label || "Shell";
  pane.element.classList.toggle("active", activeWindow?.focused_block_id === id);
  place(pane.element, rect);
}

function firstBlock(node: Layout): string | null {
  return node.split ? firstBlock(node.split.before) : node.block_id ?? null;
}

function placeDivider(divider: HTMLElement, node: Layout) {
  const { direction, ratio } = node.split!;
  const [x, y, width, height] = node.rect;
  const horizontal = direction === "horizontal";
  place(divider, horizontal ? [x + width * ratio, y, 0, height] : [x, y + height * ratio, width, 0]);
  Object.assign(divider.style, horizontal ?
    { width: "9px", marginLeft: "-4px", cursor: "col-resize" } :
    { height: "9px", marginTop: "-4px", cursor: "row-resize" });
  divider.setAttribute("aria-valuenow", String(Math.round(ratio * 100)));
}

function placePreview(node: Layout) {
  if (node.split) {
    const divider = workspace.querySelector<HTMLElement>(`[data-split="${CSS.escape(node.split.split_id)}"]`);
    if (divider) placeDivider(divider, node);
    placePreview(node.split.before);
    placePreview(node.split.after);
  } else if (node.block_id) {
    const pane = panes.get(node.block_id);
    if (pane) place(pane.element, node.rect);
  }
}

function renderLayout(node: Layout, blocks: Map<string, Block>, keep: Set<string>) {
  if (!node.split) {
    const block = node.block_id && blocks.get(node.block_id);
    if (block) mountPane(block, node.rect, keep);
    return;
  }

  const { before, after, direction, ratio, split_id } = node.split;
  renderLayout(before, blocks, keep);
  renderLayout(after, blocks, keep);

  const horizontal = direction === "horizontal";
  const [x, y, width, height] = node.rect;
  const divider = document.createElement("div");
  divider.className = "divider";
  divider.dataset.split = split_id;
  divider.tabIndex = 0;
  divider.setAttribute("role", "separator");
  divider.setAttribute("aria-label", "Resize split");
  divider.setAttribute("aria-orientation", horizontal ? "vertical" : "horizontal");
  placeDivider(divider, node);

  const resize = async (next: number) => {
    const beforeId = firstBlock(before);
    const afterId = firstBlock(after);
    if (beforeId && afterId) {
      await act("resize-split", {
        before: beforeId, after: afterId, direction,
        ratio: Math.min(.95, Math.max(.05, next)),
      });
      // Reconcile the preview with the daemon, including a rejected resize.
      if (!dragging) render();
    }
  };
  let pointerId: number | undefined;
  let previewFrame: number | undefined;
  let previewRatio = ratio;
  const paintPreview = () => {
    previewFrame = undefined;
    placePreview(previewSplit(node, previewRatio));
  };
  const pointerRatio = (event: PointerEvent) => {
    const box = workspace.getBoundingClientRect();
    return Math.min(.95, Math.max(.05, horizontal ? ((event.clientX - box.left) / box.width - x) / width :
      ((event.clientY - box.top) / box.height - y) / height));
  };
  const endDrag = (commit = false) => {
    if (pointerId === undefined) return;
    const captured = pointerId;
    pointerId = undefined;
    if (previewFrame !== undefined) cancelAnimationFrame(previewFrame);
    previewFrame = undefined;
    if (!commit) placePreview(node);
    dragging = false;
    document.body.classList.remove("resizing-columns", "resizing-rows");
    window.removeEventListener("blur", cancelDrag);
    if (divider.hasPointerCapture(captured)) divider.releasePointerCapture(captured);
  };
  const cancelDrag = () => endDrag();
  divider.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || dragging) return;
    event.preventDefault();
    pointerId = event.pointerId;
    dragging = true;
    document.body.classList.add(horizontal ? "resizing-columns" : "resizing-rows");
    divider.focus({ preventScroll: true });
    divider.setPointerCapture(event.pointerId);
    window.addEventListener("blur", cancelDrag);
  });
  divider.addEventListener("pointermove", (event) => {
    if (event.pointerId !== pointerId) return;
    event.preventDefault();
    previewRatio = pointerRatio(event);
    if (previewFrame === undefined) previewFrame = requestAnimationFrame(paintPreview);
  });
  divider.addEventListener("lostpointercapture", cancelDrag);
  divider.addEventListener("pointercancel", cancelDrag);
  divider.addEventListener("pointerup", (event) => {
    if (event.pointerId !== pointerId) return;
    event.preventDefault();
    previewRatio = pointerRatio(event);
    if (previewFrame !== undefined) cancelAnimationFrame(previewFrame);
    paintPreview();
    endDrag(true);
    void resize(previewRatio);
  });
  divider.addEventListener("keydown", (event) => {
    const step = event.key === (horizontal ? "ArrowLeft" : "ArrowUp") ? -.05 :
      event.key === (horizontal ? "ArrowRight" : "ArrowDown") ? .05 : 0;
    if (step) {
      event.preventDefault();
      void resize(ratio + step);
    }
  });
  workspace.append(divider);
}

function render() {
  const previousBlock = activeWindow?.focused_block_id;
  activeWindow = view?.windows?.find((entry) => entry.window_id === view?.active_window_id);
  windows.replaceChildren(...(view?.windows ?? []).map((entry) => {
    const button = document.createElement("button");
    button.className = "window-tab";
    const label = document.createElement("span");
    label.textContent = entry.label || "Terminal";
    button.title = label.textContent;
    button.append(icon("terminal"), label);
    button.setAttribute("aria-current", String(entry === activeWindow));
    button.addEventListener("click", () => void act("focus-window", { window: entry.window_id }));
    return button;
  }));
  const focusedSplit = (document.activeElement as HTMLElement | null)?.dataset.split;
  for (const divider of workspace.querySelectorAll(".divider")) divider.remove();
  const keep = new Set<string>();
  for (const layer of activeWindow?.layers ?? []) {
    if (layer.kind === "tiled" && layer.layout) {
      renderLayout(layer.layout, new Map(layer.blocks?.map((block) => [block.block_id, block])), keep);
    }
  }
  for (const [id, pane] of panes) {
    if (keep.has(id)) continue;
    pane.terminal?.dispose();
    pane.element.remove();
    panes.delete(id);
  }
  workspace.querySelector(".empty-state")?.remove();
  if (!keep.size) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    const label = document.createElement("p");
    label.textContent = view ? "Open a terminal to get started" : "Your sessions, ready when you are";
    const button = document.createElement("button");
    button.className = "primary";
    button.textContent = view ? "New tab" : "New session";
    button.addEventListener("click", () => runCommand(view ? "new-window" : "create"));
    empty.append(icon("terminal"), label, button);
    workspace.append(empty);
  }
  if (focusedSplit) {
    workspace.querySelector<HTMLElement>(`[data-split="${CSS.escape(focusedSplit)}"]`)?.focus();
  }
  document.querySelectorAll<HTMLButtonElement>("[data-session]").forEach((button) => {
    button.disabled = !view;
  });
  if (previousBlock !== activeWindow?.focused_block_id) { focusTerminal(); runSearch(); }
}

function runCommand(op: string, block = activeWindow?.focused_block_id, initialName = "") {
  chooser.close();
  if(op==='upload-file'){uploadInput.click();return;}
  if(op==='previous-prompt'||op==='next-prompt'){
    if(!panes.get(block??'')?.terminal?.navigatePrompt(op==='previous-prompt'))status.textContent='No retained prompt in that direction.';
    focusTerminal();return;
  }
  if(op==='export-text'||op==='export-html'||op==='export-vt'){void exportFocused(op.slice(7) as 'text'|'html'|'vt');return;}
  if (op === "create" || op === "rename" || op === "destroy" || op === "close-block") {
    const naming = op === "create" || op === "rename";
    const label = op === "create" ? "New session" : op === "rename" ? "Rename session" :
      op === "destroy" ? "End session?" : "Close pane?";
    document.querySelector("#action-title")!.textContent = label;
    document.querySelector("#action-description")!.textContent = naming ?
      (op === "create" ? "Start a new terminal workspace." : "Choose a name for this workspace.") :
      (op === "destroy" ? `This will end “${view?.label || "this session"}” and its running programs.` :
        "This will close this terminal and its running program.");
    document.querySelector<HTMLElement>("#action-label")!.hidden = !naming;
    actionName.hidden = !naming;
    actionName.disabled = !naming;
    actionName.value = op === "rename" ? view?.label ?? "" : initialName;
    const submit = document.querySelector<HTMLButtonElement>("#action-submit")!;
    submit.textContent = op === "create" ? "Create session" : op === "rename" ? "Save name" :
      op === "destroy" ? "End session" : "Close pane";
    submit.classList.toggle("danger", !naming);
    const targetSession = sessionId;
    pendingAction = () => void act(op, { session: targetSession, block, ...(naming ? { label: actionName.value.trim() } : {}) });
    actionDialog.showModal();
    if (naming) { actionName.focus(); actionName.select(); }
    else actionDialog.querySelector<HTMLButtonElement>("[data-dismiss]")!.focus();
    return;
  }
  if (op === "split-right" || op === "split-down") {
    void act("split", { block, direction: op === "split-right" ? "horizontal" : "vertical" });
  } else void act(op);
}

type Choice = { label: string; glyph: string; run: () => void; current?: boolean; hint?: string; danger?: boolean };

function renderChooser() {
  const query = filter.value.trim().toLocaleLowerCase();
  chooserItems.replaceChildren();
  let choices: Choice[];
  if (chooserMode === "sessions") {
    choices = sessionList.map((session) => ({
      label: session.label || "Shell", glyph: "terminal", current: session.session_id === sessionId,
      hint: session.session_id === sessionId ? "Current" : undefined,
      run: () => { chooser.close(); selectSession(session.session_id); void refresh(); },
    }));
  } else {
    choices = [
      { label: "Change session", glyph: "session", run: () => openChooser("sessions"), hint: "⌘K / Ctrl+K" },
      { label: "New session", glyph: "plus", run: () => runCommand("create") },
      ...(view ? [
        { label: "New tab", glyph: "plus", run: () => runCommand("new-window") },
        { label: rendererChoice === 'webgpu' ? 'Use Canvas renderer (reload)' : 'Try WebGPU compositor (reload)', glyph: 'terminal', run: () => chooseRenderer(rendererChoice === 'webgpu' ? 'canvas' : 'webgpu') },
        { label: bidiEnabled ? 'Disable RTL/BiDi (reload)' : 'Enable RTL/BiDi (reload)', glyph: 'terminal', run: chooseBidi },
        ...(uploadsEnabled?[{label:'Upload files…',glyph:'plus',run:()=>runCommand('upload-file')}]:[]),
        { label: "Previous prompt", glyph: "terminal", run: () => runCommand("previous-prompt") },
        { label: "Next prompt", glyph: "terminal", run: () => runCommand("next-prompt") },
        { label: "Export terminal as text", glyph: "terminal", run: () => runCommand("export-text") },
        { label: "Export terminal as HTML", glyph: "terminal", run: () => runCommand("export-html") },
        { label: "Export terminal as VT", glyph: "terminal", run: () => runCommand("export-vt") },
        { label: "Rename session", glyph: "session", run: () => runCommand("rename") },
        { label: "End session", glyph: "close", run: () => runCommand("destroy"), danger: true },
      ] : []),
      ...(activeWindow?.focused_block_id ? [
        { label: "Split right", glyph: "right", run: () => runCommand("split-right") },
        { label: "Split down", glyph: "down", run: () => runCommand("split-down") },
        { label: "Close pane", glyph: "close", run: () => runCommand("close-block"), danger: true },
      ] : []),
    ];
  }
  choices = choices.filter((choice) => choice.label.toLocaleLowerCase().includes(query));
  const appendChoice = (choice: Choice) => {
    const button = document.createElement("button");
    button.className = "chooser-item";
    if (choice.danger) button.classList.add("danger");
    if (choice.current) button.setAttribute("aria-current", "true");
    const label = document.createElement("span");
    label.textContent = choice.label;
    button.append(icon(choice.glyph), label);
    if (choice.hint) {
      const hint = document.createElement("small");
      hint.textContent = choice.hint;
      button.append(hint);
    }
    button.addEventListener("click", choice.run);
    chooserItems.append(button);
  };
  choices.forEach(appendChoice);
  if (chooserMode === "sessions") {
    const divider = document.createElement("div");
    divider.className = "chooser-divider";
    chooserItems.append(divider);
    appendChoice({ label: query ? `Create “${filter.value.trim()}”` : "New session", glyph: "plus",
      run: () => runCommand("create", undefined, filter.value.trim()) });
    if (!query && view) {
      appendChoice({ label: "Rename session", glyph: "session", run: () => runCommand("rename") });
      appendChoice({ label: "End session", glyph: "close", run: () => runCommand("destroy"), danger: true });
    }
  } else if (!choices.length) {
    const empty = document.createElement("p");
    empty.className = "chooser-empty";
    empty.textContent = "No matching commands";
    chooserItems.append(empty);
  }
}

function openChooser(mode: typeof chooserMode) {
  if (actionDialog.open) return;
  chooserMode = mode;
  document.querySelector("#chooser-title")!.textContent = mode === "sessions" ? "Sessions" : "Commands";
  document.querySelector("#chooser-hint")!.textContent = mode === "sessions" ? "Select a session to resume it" : "↑ ↓ to navigate · enter to run";
  filter.placeholder = mode === "sessions" ? "Filter or create…" : "Type a command…";
  filter.setAttribute("aria-label", mode === "sessions" ? "Filter sessions" : "Filter commands");
  filter.value = "";
  renderChooser();
  if (!chooser.open) chooser.showModal();
  filter.focus();
}

filter.addEventListener("input", renderChooser);
chooser.addEventListener("keydown", (event) => {
  const buttons = [...chooserItems.querySelectorAll<HTMLButtonElement>("button")];
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    if (!buttons.length) return;
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === "ArrowDown" ? (index + 1) % buttons.length :
      (index < 0 ? buttons.length - 1 : (index - 1 + buttons.length) % buttons.length);
    buttons[next]?.focus();
  } else if (event.key === "Enter" && document.activeElement === filter) {
    event.preventDefault();
    buttons[0]?.click();
  }
});
document.querySelector("#action-form")!.addEventListener("submit", (event) => {
  event.preventDefault();
  const action = pendingAction;
  actionDialog.close();
  action?.();
});
for (const dialog of [chooser, actionDialog]) {
  dialog.querySelectorAll("[data-dismiss]").forEach((button) => button.addEventListener("click", () => dialog.close()));
  dialog.addEventListener("click", (event) => { if (event.target === dialog) {
    const box = dialog.getBoundingClientRect();
    if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) dialog.close();
  } });
  dialog.addEventListener("close", () => { if (dialog === actionDialog) pendingAction = undefined; focusTerminal(); });
}
document.querySelector("#session-switcher")!.addEventListener("click", () => openChooser("sessions"));
document.querySelector("#command-palette")!.addEventListener("click", () => openChooser("commands"));
document.querySelectorAll<HTMLButtonElement>("[data-op]").forEach((button) => {
  button.addEventListener("click", () => runCommand(button.dataset.op!));
});
document.addEventListener("keydown", (event) => {
  if (event.isComposing || event.keyCode === 229) return;
  if (!(event.metaKey || event.ctrlKey) || event.altKey) return;
  const key = event.key.toLowerCase();
  if(key==='f'&&!event.shiftKey&&!chooser.open&&!actionDialog.open){event.preventDefault();event.stopPropagation();openSearch();return;}
  if ((key === "k" && !event.shiftKey) || (key === "p" && event.shiftKey)) {
    event.preventDefault();
    event.stopPropagation();
    openChooser(key === "k" ? "sessions" : "commands");
  }
}, true);
appearance.addEventListener("change", () => {
  for (const pane of panes.values()) pane.terminal?.setTheme(terminalTheme());
});

render();
void refresh();
setInterval(() => { if (!document.hidden) void refresh(); }, 1500);
