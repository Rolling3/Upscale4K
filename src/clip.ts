// One video in the list, and how its row looks.

import type { ClipInfo, Message } from "./api";
import { clipSummary, fileName, timeLeft } from "./format";

export type ClipState =
  | { kind: "ready" }
  | { kind: "waiting" }
  | { kind: "running"; fraction: number | null; eta: number | null; encoder: string; notes: string[] }
  | { kind: "done"; output: string; notes: string[] }
  | { kind: "skipped"; existing: string }
  | { kind: "failed"; message: Message }
  | { kind: "cancelled" };

export interface Clip {
  path: string;
  info: ClipInfo | null;
  state: ClipState;
}

/** Can be sent to ffmpeg: readable and not already saved. */
export function isRunnable(clip: Clip): boolean {
  return clip.state.kind !== "done" && !clip.info?.error;
}

interface StatusView {
  text: string;
  tone: "neutral" | "busy" | "ok" | "warn" | "error";
  fraction: number | null;
  notes: string[];
  noteTone: "muted" | "error";
}

function statusView(clip: Clip): StatusView {
  const s = clip.state;
  const none = { fraction: null, notes: [] as string[], noteTone: "muted" as const };
  switch (s.kind) {
    case "ready":
      if (clip.info?.error) {
        const notes = [clip.info.error.text, clip.info.error.hint].filter((n): n is string => !!n);
        return { text: "Can't read", tone: "error", fraction: null, notes, noteTone: "error" };
      }
      return { ...none, text: clip.info ? "Ready" : "Reading…", tone: "neutral" };
    case "waiting":
      return { ...none, text: "Waiting", tone: "neutral" };
    case "running": {
      const pct = s.fraction === null ? null : Math.floor(s.fraction * 100);
      const left = timeLeft(s.eta);
      const text = pct === null ? "Upscaling…" : `Upscaling · ${pct}%${left ? ` · ${left}` : ""}`;
      return { text, tone: "busy", fraction: s.fraction ?? 0, notes: s.notes, noteTone: "muted" };
    }
    case "done":
      return { text: `Saved · ${fileName(s.output)}`, tone: "ok", fraction: 1, notes: s.notes, noteTone: "muted" };
    case "skipped":
      return { ...none, text: `Skipped · ${fileName(s.existing)} already exists`, tone: "warn" };
    case "failed": {
      const notes = [s.message.text, s.message.hint].filter((n): n is string => !!n);
      return { text: "Failed", tone: "error", fraction: null, notes, noteTone: "error" };
    }
    case "cancelled":
      return { ...none, text: "Cancelled", tone: "warn" };
  }
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function removeIcon(): SVGSVGElement {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(ns, "path");
  path.setAttribute("d", "M4 4l8 8M12 4l-8 8");
  svg.appendChild(path);
  return svg;
}

/** Build a row for `clip`. `onRemove` is offered only while the clip isn't being processed. */
export function clipRow(clip: Clip, onRemove: (() => void) | null): HTMLLIElement {
  const name = fileName(clip.path);
  const row = el("li", "clip");
  row.dataset.path = clip.path;

  const main = el("div", "clip-main");
  const text = el("div", "clip-text");
  const title = el("p", "clip-name", name);
  title.title = clip.path;
  text.append(title, el("p", "clip-meta", clip.info && !clip.info.error ? clipSummary(clip.info) : ""));
  main.append(text, el("p", "clip-status"));

  if (onRemove) {
    const remove = el("button", "icon-button");
    remove.type = "button";
    remove.setAttribute("aria-label", `Remove ${name}`);
    remove.title = "Remove from list";
    remove.appendChild(removeIcon());
    remove.addEventListener("click", onRemove);
    main.appendChild(remove);
  }

  const bar = el("div", "bar");
  bar.setAttribute("role", "progressbar");
  bar.setAttribute("aria-label", `Progress for ${name}`);
  bar.setAttribute("aria-valuemin", "0");
  bar.setAttribute("aria-valuemax", "100");
  bar.appendChild(el("span", "bar-fill"));

  row.append(main, bar, el("div", "clip-notes"));
  updateRow(row, clip);
  return row;
}

/** Refresh the parts of a row that change while a batch runs. */
export function updateRow(row: HTMLElement, clip: Clip): void {
  const view = statusView(clip);
  row.dataset.tone = view.tone;
  const status = row.querySelector<HTMLElement>(".clip-status");
  if (status) status.textContent = view.text;

  const bar = row.querySelector<HTMLElement>(".bar");
  const fill = row.querySelector<HTMLElement>(".bar-fill");
  if (bar && fill) {
    bar.hidden = view.fraction === null;
    const value = Math.round((view.fraction ?? 0) * 100);
    bar.setAttribute("aria-valuenow", String(value));
    fill.style.transform = `scaleX(${view.fraction ?? 0})`;
  }

  const notes = row.querySelector<HTMLElement>(".clip-notes");
  if (notes) {
    notes.replaceChildren(...view.notes.map((n) => el("p", `clip-note ${view.noteTone}`, n)));
    notes.hidden = view.notes.length === 0;
  }
}
