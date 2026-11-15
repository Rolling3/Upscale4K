// Small text helpers for the interface.

import type { ClipInfo } from "./api";

/** Last part of a Windows or macOS path. */
export function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** 65 -> "1:05", 3725 -> "1:02:05". */
export function clock(totalSeconds: number): string {
  const s = Math.max(0, Math.round(totalSeconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

/** "about 3 min left", "less than a minute left". */
export function timeLeft(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return "";
  if (seconds < 60) return "less than a minute left";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `about ${minutes} min left`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest ? `about ${hours} h ${rest} min left` : `about ${hours} h left`;
}

function fps(rate: number): string {
  const rounded = Math.round(rate * 100) / 100;
  return Number.isInteger(rounded) ? `${rounded} fps` : `${rounded.toFixed(2)} fps`;
}

/** "1920×1080 · 60 fps · 0:05 → 3840×2160". */
export function clipSummary(info: ClipInfo): string {
  const parts: string[] = [];
  if (info.width && info.height) parts.push(`${info.width}×${info.height}`);
  if (info.frameRate) parts.push(fps(info.frameRate));
  if (info.durationSecs) parts.push(clock(info.durationSecs));
  const source = parts.join(" · ");
  const target = info.outWidth && info.outHeight ? `${info.outWidth}×${info.outHeight}` : "";
  return target ? `${source} → ${target}` : source;
}

/** "ffmpeg version 9.0.2-full_build-www.gyan.dev Copyright ..." -> "9.0.2". */
export function ffmpegVersion(line: string | null): string | null {
  if (!line) return null;
  const match = /ffmpeg version (\S+)/.exec(line);
  const version = match?.[1];
  if (!version) return null;
  return version.split("-")[0] || version;
}

/** Keep the start and the end of a long path: "C:\Users\me\…\Videos\4K". */
export function shortPath(path: string, max = 46): string {
  // Count characters, not UTF-16 units, so an emoji in a folder name is never cut in half.
  const chars = Array.from(path);
  if (chars.length <= max) return path;
  const keep = max - 1;
  const head = Math.ceil(keep * 0.4);
  const tail = keep - head;
  return `${chars.slice(0, head).join("")}…${chars.slice(chars.length - tail).join("")}`;
}

export function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}
