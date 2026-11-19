// What the review panel does with a key and what it shows, kept free of UI code so both can be tested.
import { changeLabel, changes, sourceLabel, summarize } from "../lib/diff";
import type { PresetInfo, Segment } from "../lib/types";
import { canUse, footerAction, keptWording, offerToKeep, type FooterAction, type Review } from "./review";

type Change = Extract<Segment, { type: "change" }>;

/** What a key press asks the panel to do. */
export type Action =
  | { type: "close" }
  | { type: "apply" }
  | { type: "cycle"; step: 1 | -1 }
  | { type: "toggle"; id: number }
  | { type: "startSteering" }
  /** Esc while typing an instruction: drop it and close the box. */
  | { type: "cancelSteering" }
  /** ↩ while typing an instruction: run the current style with it. */
  | { type: "submitSteering" }
  /** A key the keyboard tap took from the app before the instruction box had focus. */
  | { type: "typeSteering"; key: string }
  | { type: "runPreset"; presetId: string }
  /** R: the current style again, from the AI. */
  | { type: "retry" }
  | { type: "copy" }
  | { type: "keep"; word: string }
  | { type: "openSettings" };

/** The action, if any, and whether the key is marked handled so the web view does nothing more with it. */
export interface KeyOutcome {
  action: Action | null;
  preventDefault: boolean;
}

/** What decides what a key does. */
export interface KeyState {
  /** A notice shows, and no review. */
  notice: boolean;
  /** A review shows. */
  review: boolean;
  steering: boolean;
  presets: readonly PresetInfo[];
  focusedId: number | null;
  /** The word "Always keep" offers, if any. */
  keepOffer: string | null;
}

export type KeyPress = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey">;

const handled = (action: Action | null): KeyOutcome => ({ action, preventDefault: true });

/**
 * What a key does in the panel: one typed into the panel, or one the keyboard tap took from the app
 * (`forwarded`). Null when the key does nothing.
 */
export function keyAction(state: KeyState, e: KeyPress, forwarded = false): KeyOutcome | null {
  if (state.notice) {
    // Marked handled, or WebKit passes the key back to GTK after the panel has hidden, and on
    // Linux GTK then drops the next press of that key: the first one in the next panel.
    return e.key === "Escape" || e.key === "Enter" ? handled({ type: "close" }) : null;
  }
  if (!state.review) return null;
  const mod = e.metaKey || e.ctrlKey;
  if (state.steering) {
    if (e.key === "Escape") return handled({ type: "cancelSteering" });
    if (e.key === "Enter") return handled({ type: "submitSteering" });
    // Typed after / but before the panel had focus.
    if (forwarded && !mod && (e.key === "Backspace" || e.key.length === 1)) return { action: { type: "typeSteering", key: e.key }, preventDefault: false };
    return null;
  }
  if (mod && /^[1-9]$/.test(e.key)) {
    const preset = state.presets[Number(e.key) - 1];
    return handled(preset ? { type: "runPreset", presetId: preset.id } : null);
  }
  switch (e.key) {
    case "Enter":
      return handled({ type: "apply" });
    case "Escape":
      return handled({ type: "close" });
    case "Tab":
      return handled({ type: "cycle", step: e.shiftKey ? -1 : 1 });
    case "ArrowRight":
    case "ArrowDown":
      return handled({ type: "cycle", step: 1 });
    case "ArrowLeft":
    case "ArrowUp":
      return handled({ type: "cycle", step: -1 });
    case " ":
    case "Backspace":
    case "Delete":
      return state.focusedId != null ? handled({ type: "toggle", id: state.focusedId }) : null;
    case "/":
      return handled({ type: "startSteering" });
  }
  if (mod && e.key === "c") return handled({ type: "copy" });
  if (e.key === "r") return handled({ type: "retry" });
  if (!mod && e.key === "a" && state.keepOffer) return handled({ type: "keep", word: state.keepOffer });
  if (mod && e.key === ",") return handled({ type: "openSettings" });
  if (!mod && /^[1-9]$/.test(e.key)) {
    const preset = state.presets[Number(e.key) - 1];
    // Not marked handled: a plain digit was never prevented, unlike ⌘1–⌘9.
    if (preset) return { action: { type: "runPreset", presetId: preset.id }, preventDefault: false };
  }
  return null;
}

/** What decides what a review shows. */
export interface ViewState {
  review: Review;
  canApply: boolean;
  showTiming: boolean;
  presets: readonly PresetInfo[];
  presetId: string;
  rejected: ReadonlySet<number>;
  focusedId: number | null;
  /** Words added to the word list from this panel. */
  added: ReadonlySet<string>;
  /** The panel takes the review keys, so it names them. */
  keyboard: boolean;
  showTips: boolean;
  /** The next apply replaces without the clipboard put aside. */
  anyway: boolean;
}

/** The panel shows only as many style chips as fit; see `PanelView.chips`. */
export const CHIPS = 7;

export interface Chip {
  id: string;
  name: string;
  /** The digit that picks it. */
  key: number;
  active: boolean;
}

export interface PanelView {
  chips: Chip[];
  changes: Change[];
  focusedChange: Change | null;
  /** The note under the text for the focused change. */
  note: { label: string; text: string } | null;
  /** The finished answer changes nothing. */
  unchanged: boolean;
  /** The engine is still answering. */
  busy: boolean;
  /** A finished result that can be applied or copied. */
  usable: boolean;
  keptTerms: string[];
  keepOffer: string | null;
  /** What goes where the replace key goes. */
  action: FooterAction;
  /** "SuperEditr can't replace text here." */
  copyOnly: boolean;
  /** The line above the footer: replace anyway, the key tips, or nothing. */
  banner: "anyway" | "tips" | null;
  status: {
    dot: "pulsing" | "bad" | "good";
    text: string;
    /** The footer's hover text. */
    title: string;
    /** Who answered, once the answer is in; empty otherwise. */
    source: string;
    /** How long it took, when the settings show it; empty otherwise. */
    timing: string;
    /** An earlier result of the same selection, shown again: no model ran. */
    fromEarlier: boolean;
  };
  /** The keys the footer names, beside the replace key and esc. */
  keys: { next: boolean; toggle: "restore" | "keep original" | null; instruct: boolean };
}

export function viewOf(s: ViewState): PanelView {
  const { review, rejected, focusedId, keyboard } = s;
  const { view, phase, source, timing, remembered } = review;
  const list = changes(view);
  const focusedChange = list.find((c) => c.id === focusedId) ?? null;
  const unchanged = phase === "done" && list.length === 0;
  const busy = phase === "local" || phase === "streaming" || phase === "idle";
  return {
    chips: s.presets.slice(0, CHIPS).map((p, i) => ({ id: p.id, name: p.name, key: i + 1, active: p.id === s.presetId })),
    changes: list,
    focusedChange,
    note: focusedChange && {
      label: changeLabel(focusedChange),
      text: focusedChange.note ?? (rejected.has(focusedChange.id) ? "Keeping your original." : `“${focusedChange.old.trim()}” → “${focusedChange.new.trim()}”`),
    },
    unchanged,
    busy,
    usable: canUse(review),
    keptTerms: keptWording(review, rejected),
    keepOffer: offerToKeep(review, rejected, focusedId, s.added),
    action: footerAction(review, s.canApply),
    copyOnly: !s.canApply && !unchanged,
    banner: s.anyway ? "anyway" : s.showTips && keyboard && phase === "done" && list.length ? "tips" : null,
    status: {
      dot: busy ? "pulsing" : phase === "failed" ? "bad" : "good",
      text: busy
        ? phase === "streaming" && source
          ? `${source}…`
          : "Checking…"
        : phase === "failed"
          ? "Couldn’t finish"
          : list.length
            ? `${list.length} change${list.length === 1 ? "" : "s"}`
            : "No changes",
      title: [summarize(view), sourceLabel(source, view), remembered ? "from earlier" : timing].filter(Boolean).join(" · "),
      source: phase === "done" && source ? sourceLabel(source.replace(" (backup)", ""), view) : "",
      timing: s.showTiming ? timing : "",
      fromEarlier: phase === "done" && remembered,
    },
    keys: {
      next: keyboard && list.length > 1,
      toggle: keyboard && focusedId != null ? (rejected.has(focusedId) ? "restore" : "keep original") : null,
      instruct: keyboard,
    },
  };
}
