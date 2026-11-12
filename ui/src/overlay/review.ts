// The overlay's review state, kept free of UI code so its transitions can be tested.
import { changes } from "../lib/diff";
import type { DiffView, EngineEvent, KeptTerm } from "../lib/types";

export type Phase = "idle" | "local" | "streaming" | "done" | "failed";

export interface Review {
  /** What the overlay shows. */
  view: DiffView | null;
  /** The offline checker's complete result, kept so a failed model run can fall back to it. */
  localView: DiffView | null;
  /** True only when `view` is a finished result that is safe to apply or copy. */
  complete: boolean;
  phase: Phase;
  source: string;
  timing: string;
  error: string;
  changedTokens: string[];
  /** The author's own terms a rewrite would have dropped; those changes start out rejected. */
  keptTerms: KeptTerm[];
  /** Why Apple Intelligence stood in for the chosen AI. */
  backupFor: string;
  /** Words the panel may offer to add to the word list. */
  learnable: string[];
  /** The result of an earlier review of the same selection, shown again: no AI ran just now. */
  remembered: boolean;
}

/** What running a style asks the backend. */
export interface RunRequest {
  presetId: string;
  /** The typed instruction, or null for none. A result with one is never shown again. */
  steer: string | null;
  /** Ask the AI even when this selection already has a result for the style, as R does. Otherwise
   *  switching to a style answered a moment ago shows that result again. */
  fresh: boolean;
}

export function runRequest(presetId: string, instruction: string | undefined, fresh: boolean): RunRequest {
  return { presetId, steer: instruction?.trim() || null, fresh };
}

export function emptyReview(): Review {
  return { view: null, localView: null, complete: false, phase: "idle", source: "", timing: "", error: "", changedTokens: [], keptTerms: [], backupFor: "", learnable: [], remembered: false };
}

/** `remembered`: the event is an earlier review's result, shown again, so it has no timing of its own. */
export function reduce(review: Review, event: EngineEvent, remembered = false): Review {
  switch (event.type) {
    case "local":
      // The local result is complete on its own; it shows until the model's version streams in.
      if (review.phase !== "idle") return { ...review, localView: event.view };
      return { ...review, view: event.view, localView: event.view, complete: true, phase: "local", source: "Instant check", timing: "" };
    case "partial":
      return { ...review, view: event.view, complete: false, phase: "streaming", source: event.source, timing: "" };
    case "done":
      return {
        ...review,
        view: event.view,
        complete: true,
        phase: "done",
        source: event.source,
        timing: remembered ? "" : seconds(event.totalMs),
        changedTokens: event.changedTokens,
        keptTerms: event.keptTerms,
        backupFor: event.backupFor ?? "",
        learnable: event.learnable,
        remembered,
      };
    case "failed":
      // Never leave a partial revision applicable. Fall back to the offline result when there is one.
      if (review.localView) {
        return { ...review, view: review.localView, complete: true, phase: "done", source: "Instant check", error: event.message };
      }
      return { ...review, complete: false, phase: "failed", error: event.message };
  }
}

/** "0.8 s": one number is enough for the user; the split into first words and total isn't. */
export function seconds(ms: number): string {
  return `${(ms / 1000).toFixed(1)} s`;
}

/**
 * The word to offer "Always keep" for: an unknown word in a change the author rejected (␣), or one
 * that started out rejected to keep their wording. The focused change comes first.
 */
export function offerToKeep(
  review: Review,
  rejected: ReadonlySet<number>,
  focusedId: number | null,
  added: ReadonlySet<string>,
): string | null {
  if (review.phase !== "done") return null;
  const list = changes(review.view)
    .filter((c) => rejected.has(c.id))
    .sort((a, b) => Number(b.id === focusedId) - Number(a.id === focusedId));
  for (const change of list) {
    const words = change.old.split(/[^\p{L}\p{N}-]+/u);
    const word = review.learnable.find((w) => words.includes(w) && !added.has(w));
    if (word) return word;
  }
  return null;
}

/** The author's terms still kept: those with a change that drops them still rejected. */
export function keptWording(review: Review, rejected: ReadonlySet<number>): string[] {
  return review.keptTerms.filter((k) => k.changes.some((id) => rejected.has(id))).map((k) => k.term);
}

export function canUse(review: Review): boolean {
  return review.complete && review.view !== null;
}

/**
 * A replacement in flight, and what the apply key does next. When the backend couldn't put the
 * user's clipboard aside (Windows), it brings the review back and the next apply replaces anyway,
 * leaving the fix on the clipboard.
 */
export interface Replacing<S extends { id: number }> {
  /** The review being applied, kept so it can come back. */
  applying: S | null;
  /** The next apply goes ahead without the clipboard put aside. */
  anyway: boolean;
}

export function notReplacing<S extends { id: number }>(): Replacing<S> {
  return { applying: null, anyway: false };
}

/** The apply key was pressed in `session`: what to ask the backend for. */
export function startReplacing<S extends { id: number }>(state: Replacing<S>, session: S): { state: Replacing<S>; anyway: boolean } {
  return { state: { applying: session, anyway: state.anyway }, anyway: state.anyway };
}

/**
 * The backend offered to replace anyway for review `id`: the review to show again, or null when
 * the offer is for a review that isn't the one being applied.
 */
export function offerAnyway<S extends { id: number }>(state: Replacing<S>, id: number): { state: Replacing<S>; session: S } | null {
  if (!state.applying || state.applying.id !== id) return null;
  return { state: { applying: null, anyway: true }, session: state.applying };
}
