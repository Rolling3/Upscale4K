// Typed calls into the Rust side.

import { Channel, invoke } from "@tauri-apps/api/core";

export interface Message {
  text: string;
  hint: string | null;
}

export interface EncoderInfo {
  label: string;
  hardware: boolean;
}

export interface Status {
  version: string;
  platform: "windows" | "macos" | "other";
  ffmpegFound: boolean;
  ffmpegVersion: string | null;
  ffmpegPath: string | null;
  encoder: EncoderInfo | null;
  install: { command: string; canRunInApp: boolean; note: string | null };
  outputFolder: string;
}

export interface ClipInfo {
  path: string;
  width: number | null;
  height: number | null;
  frameRate: number | null;
  durationSecs: number | null;
  outWidth: number | null;
  outHeight: number | null;
  error: Message | null;
}

export type ConflictChoice = "keepBoth" | "replace" | "skip";

export type BatchEvent =
  | { event: "fileStarted"; data: { index: number; output: string; encoder: EncoderInfo; notes: string[] } }
  | { event: "fileProgress"; data: { index: number; fraction: number | null; etaSecs: number | null; speed: number | null } }
  | { event: "fileDone"; data: { index: number; output: string; note: string | null } }
  | { event: "fileSkipped"; data: { index: number; existing: string } }
  | { event: "fileFailed"; data: { index: number; message: Message } }
  | { event: "fileCancelled"; data: { index: number } }
  | { event: "folderFailed"; data: { message: Message } }
  | { event: "finished"; data: { saved: number; failed: number; skipped: number; cancelled: boolean } }
  | { event: "aborted"; data: { message: Message } };

/** Errors from commands arrive either as a Message or as plain text. */
export function asMessage(error: unknown): Message {
  if (typeof error === "object" && error !== null && "text" in error) {
    const e = error as { text: unknown; hint?: unknown };
    return { text: String(e.text), hint: typeof e.hint === "string" ? e.hint : null };
  }
  return { text: String(error), hint: null };
}

export const api = {
  checkEnvironment: () => invoke<Status>("check_environment"),
  pickVideos: () => invoke<string[]>("pick_videos"),
  filterVideos: (paths: string[]) => invoke<{ accepted: string[]; rejected: string[] }>("filter_videos", { paths }),
  inspectVideos: (paths: string[]) => invoke<ClipInfo[]>("inspect_videos", { paths }),
  findConflicts: (paths: string[]) => invoke<number[]>("find_conflicts", { paths }),
  pickOutputFolder: () => invoke<string | null>("pick_output_folder"),
  startBatch: (items: { path: string; onConflict: ConflictChoice }[], onEvent: (event: BatchEvent) => void) => {
    const channel = new Channel<BatchEvent>();
    channel.onmessage = onEvent;
    return invoke<void>("start_batch", { items, onEvent: channel });
  },
  cancelBatch: () => invoke<void>("cancel_batch"),
  openOutputFolder: () => invoke<void>("open_output_folder"),
  installFfmpeg: () => invoke<void>("install_ffmpeg"),
};
