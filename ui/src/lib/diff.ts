import type { DiffView, Segment } from "./types";

type Change = Extract<Segment, { type: "change" }>;

export function changes(view: DiffView | null): Change[] {
  return (view?.segments ?? []).filter((s): s is Change => s.type === "change");
}

/** Rebuilds text from a view, keeping the original wording for rejected changes. */
export function applyView(view: DiffView, rejected: ReadonlySet<number>): string {
  let out = "";
  for (const segment of view.segments) {
    if (segment.type === "equal") out += segment.text;
    else out += rejected.has(segment.id) ? segment.old : segment.new;
  }
  return out + view.pending;
}

export const kindLabel: Record<string, string> = {
  spelling: "Spelling",
  grammar: "Grammar",
  punctuation: "Punctuation",
  capitalization: "Capitalization",
  spacing: "Spacing",
  style: "Rewrite",
};

/** Plain-language summary like "2 spelling, 1 grammar". */
export function summarize(view: DiffView | null): string {
  const counts = new Map<string, number>();
  for (const c of changes(view)) counts.set(c.kind, (counts.get(c.kind) ?? 0) + 1);
  return [...counts.entries()].map(([kind, n]) => `${n} ${kindLabel[kind]?.toLowerCase() ?? kind}`).join(", ");
}

/** Formats an accelerator like "Control+Alt+E" as macOS key symbols. A chord pressed twice
 * ("Command+E Command+E") reads as holding the modifiers and pressing the key twice: "⌘E E". */
export function keySymbols(accelerator: string, mac = true): string {
  if (!accelerator) return "";
  const [first, second] = accelerator.trim().split(/\s+/);
  if (second && second.toLowerCase() === first.toLowerCase()) {
    const key = first.split("+").pop() ?? "";
    return `${keySymbols(first, mac)} ${key.toUpperCase()}`;
  }
  const map: Record<string, string> = mac
    ? { control: "⌃", ctrl: "⌃", alt: "⌥", option: "⌥", shift: "⇧", command: "⌘", cmd: "⌘", cmdorctrl: "⌘", super: "⌘", enter: "↩", escape: "⎋", space: "Space", tab: "⇥" }
    : { control: "Ctrl+", ctrl: "Ctrl+", alt: "Alt+", shift: "Shift+", command: "Win+", super: "Win+", cmdorctrl: "Ctrl+" };
  return accelerator
    .split("+")
    .map((part) => map[part.toLowerCase()] ?? part.toUpperCase())
    .join("");
}
