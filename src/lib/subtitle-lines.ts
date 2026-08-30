// The order the two (or three) language lines are shown in, in one place.
//
// SubtitleView renders from this and the clipboard copies from it, so what
// lands in the paste buffer is what was on screen. Two implementations of
// "translation first, source below" would eventually disagree.
import type { Lang, SubtitleUpdate } from "./types";

const LANG_ORDER: Lang[] = ["zh", "ko", "en"];

export type SubtitleLine = { lang: Lang; text: string; primary: boolean };

export function linesFor(update: SubtitleUpdate): SubtitleLine[] {
  const out: SubtitleLine[] = [];
  const subs = update.subtitles;
  const src = update.sourceLang as Lang;
  // Translation first (white, prominent), source below (gray, small).
  for (const l of LANG_ORDER) {
    if (l !== src && subs[l]) out.push({ lang: l, text: subs[l]!, primary: false });
  }
  if (subs[src]) out.push({ lang: src, text: subs[src]!, primary: true });
  return out;
}

/** What Ctrl+Alt+C and the copy button put on the clipboard. */
export function asClipboardText(segments: SubtitleUpdate[]): string {
  return segments
    .map((seg) => linesFor(seg).map((l) => l.text).join("\n"))
    .filter((block) => block.length > 0)
    .join("\n\n");
}
