// Upscale4K interface: list of clips, output folder, start/cancel, progress.

import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";

import { api, asMessage, type BatchEvent, type ConflictChoice, type Message, type Status } from "./api";
import { type Clip, clipRow, isRunnable, updateRow } from "./clip";
import { ffmpegVersion, fileName, plural, shortPath } from "./format";

function byId<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`missing #${id}`);
  return node as T;
}

const ui = {
  pill: byId<HTMLParagraphElement>("encoder-pill"),
  setup: byId<HTMLElement>("setup"),
  setupTitle: byId<HTMLElement>("setup-title"),
  setupText: byId<HTMLElement>("setup-text"),
  installCommand: byId<HTMLElement>("install-command"),
  installNote: byId<HTMLElement>("install-note"),
  copyCommand: byId<HTMLButtonElement>("copy-command"),
  runInstall: byId<HTMLButtonElement>("run-install"),
  recheck: byId<HTMLButtonElement>("recheck"),
  drop: byId<HTMLElement>("drop"),
  choose: byId<HTMLButtonElement>("choose"),
  notice: byId<HTMLElement>("notice"),
  noticeText: byId<HTMLElement>("notice-text"),
  noticeHint: byId<HTMLElement>("notice-hint"),
  noticeDismiss: byId<HTMLButtonElement>("notice-dismiss"),
  clips: byId<HTMLUListElement>("clips"),
  outputPath: byId<HTMLElement>("output-path"),
  changeFolder: byId<HTMLButtonElement>("change-folder"),
  openFolder: byId<HTMLButtonElement>("open-folder"),
  summary: byId<HTMLElement>("summary"),
  clear: byId<HTMLButtonElement>("clear"),
  cancel: byId<HTMLButtonElement>("cancel"),
  start: byId<HTMLButtonElement>("start"),
  about: byId<HTMLElement>("about"),
  quitDialog: byId<HTMLDialogElement>("quit-dialog"),
  dialog: byId<HTMLDialogElement>("conflict-dialog"),
  dialogTitle: byId<HTMLElement>("conflict-title"),
  dialogList: byId<HTMLUListElement>("conflict-list"),
};

let status: Status | null = null;
let checking = false;
let clips: Clip[] = [];
/** Clips of the running batch, in the order the Rust side numbers them. */
let batch: Clip[] | null = null;
let cancelling = false;
/** The output folder couldn't be used, so the batch ran nothing. */
let folderFailed = false;
const rows = new Map<Clip, HTMLLIElement>();

// ---------- notices ----------

function showNotice(message: Message | string, tone: "info" | "error" = "error"): void {
  const m = typeof message === "string" ? { text: message, hint: null } : message;
  ui.notice.dataset.tone = tone;
  ui.noticeText.textContent = m.text;
  ui.noticeHint.textContent = m.hint ?? "";
  ui.noticeHint.hidden = !m.hint;
  ui.notice.hidden = false;
}

function hideNotice(): void {
  ui.notice.hidden = true;
}

// ---------- rendering ----------

function renderStatus(): void {
  const s = status;
  if (checking || !s) {
    ui.pill.textContent = "Checking ffmpeg…";
    ui.pill.dataset.tone = "neutral";
  } else if (!s.ffmpegFound) {
    ui.pill.textContent = "ffmpeg not found";
    ui.pill.dataset.tone = "warn";
  } else if (!s.encoder) {
    ui.pill.textContent = "No H.264 encoder";
    ui.pill.dataset.tone = "warn";
  } else {
    ui.pill.textContent = `Encoder: ${s.encoder.label}`;
    ui.pill.dataset.tone = s.encoder.hardware ? "ok" : "neutral";
  }
  ui.pill.title = s?.ffmpegPath ? `ffmpeg: ${s.ffmpegPath}` : "";

  const needsSetup = !!s && !checking && (!s.ffmpegFound || !s.encoder);
  ui.setup.hidden = !needsSetup;
  if (s && needsSetup) {
    if (s.ffmpegFound) {
      ui.setupTitle.textContent = "This ffmpeg can't make H.264 video";
      ui.setupText.textContent = "The ffmpeg that was found has no H.264 encoder (libx264). Install the full build:";
    } else {
      ui.setupTitle.textContent = "ffmpeg is needed";
      ui.setupText.textContent = "Upscale4K uses ffmpeg to process videos. It isn't installed, or Upscale4K couldn't find it. Install it with:";
    }
    ui.installCommand.textContent = s.install.command;
    ui.installNote.textContent = s.install.note ?? "";
    ui.installNote.hidden = !s.install.note;
    ui.runInstall.hidden = !s.install.canRunInApp;
  }
  ui.recheck.disabled = checking;

  ui.outputPath.textContent = shortPath(s?.outputFolder ?? "");
  ui.outputPath.title = s?.outputFolder ?? "";
  const ffmpeg = ffmpegVersion(s?.ffmpegVersion ?? null);
  ui.about.textContent = s ? `Upscale4K ${s.version}${ffmpeg ? ` · ffmpeg ${ffmpeg}` : ""}` : "";
  renderActions();
}

function renderActions(): void {
  const running = batch !== null;
  const runnable = clips.filter(isRunnable).length;
  const ready = !!status?.encoder && !checking;
  ui.start.disabled = running || !ready || runnable === 0;
  ui.start.textContent = runnable > 1 ? `Upscale ${runnable} videos` : "Upscale";
  ui.start.hidden = running;
  ui.cancel.hidden = !running;
  ui.cancel.disabled = cancelling;
  ui.cancel.textContent = cancelling ? "Cancelling…" : "Cancel";
  ui.clear.disabled = clips.length === 0 || (running && clips.every((c) => batch?.includes(c)));
  ui.changeFolder.disabled = running;
  ui.drop.classList.toggle("compact", clips.length > 0);
}

function renderClips(): void {
  const running = batch;
  const next: HTMLLIElement[] = clips.map((clip) => {
    const inBatch = running?.includes(clip) ?? false;
    const row = clipRow(clip, inBatch ? null : () => removeClip(clip));
    rows.set(clip, row);
    return row;
  });
  for (const clip of [...rows.keys()]) {
    if (!clips.includes(clip)) rows.delete(clip);
  }
  ui.clips.replaceChildren(...next);
  ui.clips.hidden = clips.length === 0;
  renderActions();
}

function refresh(clip: Clip): void {
  const row = rows.get(clip);
  if (row) updateRow(row, clip);
}

// ---------- clips ----------

const pathKey = (path: string) => path.toLowerCase();

async function addPaths(paths: string[]): Promise<void> {
  if (paths.length === 0) return;
  let filtered: { accepted: string[]; rejected: string[] };
  try {
    filtered = await api.filterVideos(paths);
  } catch (error) {
    showNotice(asMessage(error));
    return;
  }
  const known = new Set(clips.map((c) => pathKey(c.path)));
  const fresh: Clip[] = [];
  for (const path of filtered.accepted) {
    if (known.has(pathKey(path))) continue;
    known.add(pathKey(path));
    fresh.push({ path, info: null, state: { kind: "ready" } });
  }
  if (filtered.rejected.length) {
    const names = filtered.rejected.slice(0, 3).map(fileName).join(", ");
    const more = filtered.rejected.length > 3 ? ` and ${filtered.rejected.length - 3} more` : "";
    showNotice(`Not added: ${names}${more}. Only MP4, MKV and MOV videos can be upscaled.`, "info");
  } else if (fresh.length) {
    hideNotice();
  }
  if (!fresh.length) return;
  clips = [...clips, ...fresh];
  ui.summary.textContent = "";
  renderClips();
  await inspect(fresh);
}

async function inspect(targets: Clip[]): Promise<void> {
  if (!targets.length) return;
  try {
    const infos = await api.inspectVideos(targets.map((c) => c.path));
    targets.forEach((clip, i) => {
      clip.info = infos[i] ?? null;
    });
  } catch (error) {
    showNotice(asMessage(error));
  }
  renderClips();
}

function removeClip(clip: Clip): void {
  clips = clips.filter((c) => c !== clip);
  renderClips();
}

function clearList(): void {
  clips = batch ? clips.filter((c) => batch?.includes(c)) : [];
  ui.summary.textContent = "";
  hideNotice();
  renderClips();
}

// ---------- environment ----------

async function checkEnvironment(): Promise<void> {
  checking = true;
  renderStatus();
  try {
    status = await api.checkEnvironment();
  } catch (error) {
    showNotice(asMessage(error));
  } finally {
    checking = false;
    renderStatus();
  }
  // Clips added while ffmpeg was missing can be read now.
  if (status?.ffmpegFound) {
    await inspect(clips.filter((c) => !c.info || c.info.error));
  }
}

async function copyCommand(): Promise<void> {
  const text = ui.installCommand.textContent ?? "";
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const field = document.createElement("textarea");
    field.value = text;
    document.body.appendChild(field);
    field.select();
    document.execCommand("copy");
    field.remove();
  }
  ui.copyCommand.textContent = "Copied";
  window.setTimeout(() => (ui.copyCommand.textContent = "Copy"), 1600);
}

async function runInstall(): Promise<void> {
  try {
    await api.installFfmpeg();
    showNotice("A terminal window opened with the winget command. When it has finished, choose Check again.", "info");
  } catch (error) {
    showNotice(asMessage(error));
  }
}

// ---------- output folder ----------

async function changeFolder(): Promise<void> {
  try {
    const folder = await api.pickOutputFolder();
    if (folder && status) {
      status = { ...status, outputFolder: folder };
      hideNotice();
      renderStatus();
    }
  } catch (error) {
    showNotice(asMessage(error));
  }
}

async function openFolder(): Promise<void> {
  try {
    await api.openOutputFolder();
  } catch (error) {
    showNotice(asMessage(error));
  }
}

// ---------- conflicts ----------

function outputName(path: string): string {
  const name = fileName(path);
  const dot = name.lastIndexOf(".");
  return `${dot > 0 ? name.slice(0, dot) : name}_4K.mp4`;
}

function askAboutConflicts(conflicting: Clip[]): Promise<ConflictChoice | null> {
  ui.dialogTitle.textContent =
    conflicting.length === 1
      ? `${outputName(conflicting[0]?.path ?? "")} already exists`
      : `${conflicting.length} videos already have a 4K version in this folder`;
  ui.dialogList.replaceChildren(
    ...conflicting.map((clip) => {
      const item = document.createElement("li");
      item.textContent = outputName(clip.path);
      return item;
    }),
  );
  ui.dialog.returnValue = "";
  ui.dialog.showModal();
  return new Promise((resolve) => {
    ui.dialog.addEventListener(
      "close",
      () => {
        const value = ui.dialog.returnValue;
        resolve(value === "keepBoth" || value === "replace" || value === "skip" ? value : null);
      },
      { once: true },
    );
  });
}

// ---------- batch ----------

/** True from the Start click until the batch is running or the start was called off. */
let starting = false;

async function start(): Promise<void> {
  if (starting || batch) return;
  starting = true;
  try {
    await startQueue();
  } finally {
    starting = false;
  }
}

async function startQueue(): Promise<void> {
  const queue = clips.filter(isRunnable);
  if (!queue.length) return;
  hideNotice();
  ui.summary.textContent = "";

  let choice: ConflictChoice = "keepBoth";
  try {
    const conflicts = await api.findConflicts(queue.map((c) => c.path));
    if (conflicts.length) {
      const picked = await askAboutConflicts(conflicts.map((i) => queue[i]).filter((c): c is Clip => !!c));
      if (!picked) return;
      choice = picked;
    }
  } catch (error) {
    showNotice(asMessage(error));
    return;
  }

  batch = queue;
  cancelling = false;
  folderFailed = false;
  for (const clip of queue) clip.state = { kind: "waiting" };
  renderClips();
  try {
    await api.startBatch(
      queue.map((c) => ({ path: c.path, onConflict: choice })),
      onBatchEvent,
    );
  } catch (error) {
    for (const clip of queue) clip.state = { kind: "ready" };
    batch = null;
    renderClips();
    showNotice(asMessage(error));
  }
}

function onBatchEvent(event: BatchEvent): void {
  const current = batch;
  if (!current) return;
  const clipAt = (index: number) => current[index];

  switch (event.event) {
    case "fileStarted": {
      const clip = clipAt(event.data.index);
      if (!clip) return;
      clip.state = { kind: "running", fraction: 0, eta: null, encoder: event.data.encoder.label, notes: event.data.notes };
      refresh(clip);
      return;
    }
    case "fileProgress": {
      const clip = clipAt(event.data.index);
      if (!clip || clip.state.kind !== "running") return;
      clip.state = { ...clip.state, fraction: event.data.fraction, eta: event.data.etaSecs };
      refresh(clip);
      return;
    }
    case "fileDone": {
      const clip = clipAt(event.data.index);
      if (!clip) return;
      // Keep the notes from the encode (for example why libx264 was used) next to the result.
      const notes = clip.state.kind === "running" ? clip.state.notes : [];
      clip.state = { kind: "done", output: event.data.output, notes: [...notes, ...(event.data.note ? [event.data.note] : [])] };
      refresh(clip);
      return;
    }
    case "fileSkipped": {
      const clip = clipAt(event.data.index);
      if (clip) clip.state = { kind: "skipped", existing: event.data.existing };
      if (clip) refresh(clip);
      return;
    }
    case "fileFailed": {
      const clip = clipAt(event.data.index);
      if (clip) clip.state = { kind: "failed", message: event.data.message };
      if (clip) refresh(clip);
      return;
    }
    case "fileCancelled": {
      const clip = clipAt(event.data.index);
      if (clip) clip.state = { kind: "cancelled" };
      if (clip) refresh(clip);
      return;
    }
    case "folderFailed":
      folderFailed = true;
      for (const clip of current) clip.state = { kind: "ready" };
      showNotice(event.data.message);
      return;
    case "finished":
      finishBatch(event.data);
      return;
    case "aborted":
      for (const clip of current) {
        if (clip.state.kind === "waiting" || clip.state.kind === "running") clip.state = { kind: "ready" };
      }
      showNotice(event.data.message);
      finishBatch(null);
      return;
  }
}

function finishBatch(result: { saved: number; failed: number; skipped: number; cancelled: boolean } | null): void {
  batch = null;
  cancelling = false;
  renderClips();
  if (!result) return;
  if (folderFailed) {
    ui.summary.textContent = "Nothing was upscaled.";
    return;
  }
  const parts: string[] = [];
  if (result.saved) parts.push(`${plural(result.saved, "video", "videos")} saved`);
  if (result.failed) parts.push(`${result.failed} failed`);
  if (result.skipped) parts.push(`${result.skipped} skipped`);
  const lead = result.cancelled ? "Cancelled." : "Done.";
  ui.summary.textContent = parts.length ? `${lead} ${parts.join(", ")}.` : result.cancelled ? "Cancelled." : "Nothing was saved.";
  if (result.saved > 0) void openFolder();
}

async function cancel(): Promise<void> {
  if (!batch || cancelling) return;
  cancelling = true;
  renderActions();
  try {
    await api.cancelBatch();
  } catch (error) {
    cancelling = false;
    renderActions();
    showNotice(asMessage(error));
  }
}

async function choose(): Promise<void> {
  try {
    await addPaths(await api.pickVideos());
  } catch (error) {
    showNotice(asMessage(error));
  }
}

// ---------- wiring ----------

ui.choose.addEventListener("click", () => void choose());
ui.start.addEventListener("click", () => void start());
ui.cancel.addEventListener("click", () => void cancel());
ui.clear.addEventListener("click", clearList);
ui.changeFolder.addEventListener("click", () => void changeFolder());
ui.openFolder.addEventListener("click", () => void openFolder());
ui.recheck.addEventListener("click", () => void checkEnvironment());
ui.copyCommand.addEventListener("click", () => void copyCommand());
ui.runInstall.addEventListener("click", () => void runInstall());
ui.noticeDismiss.addEventListener("click", hideNotice);

function listenForDrops(): void {
  try {
    void getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload;
      if (payload.type === "enter" || payload.type === "over") {
        ui.drop.classList.add("over");
      } else if (payload.type === "leave") {
        ui.drop.classList.remove("over");
      } else if (payload.type === "drop") {
        ui.drop.classList.remove("over");
        void addPaths(payload.paths);
      }
    });
  } catch (error) {
    // The rest of the window still works with the file picker.
    showNotice(`Drag and drop isn't available: ${asMessage(error).text}`, "info");
  }
}

/** The window was closed during a batch: ask before stopping it. */
function listenForCloseRequests(): void {
  ui.quitDialog.addEventListener("close", () => {
    if (ui.quitDialog.returnValue === "quit") {
      cancelling = true;
      renderActions();
      api.quitApp().catch((error) => showNotice(asMessage(error)));
    }
  });
  listen("close-requested", () => {
    if (ui.quitDialog.open) return;
    if (ui.dialog.open) ui.dialog.close();
    ui.quitDialog.returnValue = "";
    ui.quitDialog.showModal();
  }).catch(() => {
    // Without the event the window still closes through the system after a batch.
  });
}

renderClips();
renderStatus();
listenForDrops();
listenForCloseRequests();
void checkEnvironment();
