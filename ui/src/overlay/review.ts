// The overlay's review state, kept free of UI code so its transitions can be tested.
import { changes } from "../lib/diff";
import type { DiffView, EngineEvent } from "../lib/types";

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
  keptTerms: string[];
  /** Why Apple Intelligence stood in for the chosen AI. */
  backupFor: string;
  /** Words the panel may offer to add to the word list. */
  learnable: string[];
}

export function emptyReview(): Review {
  return { view: null, localView: null, complete: false, phase: "idle", source: "", timing: "", error: "", changedTokens: [], keptTerms: [], backupFor: "", learnable: [] };
}

export function reduce(review: Review, event: EngineEvent): Review {
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
        timing: seconds(event.totalMs),
        changedTokens: event.changedTokens,
        keptTerms: event.keptTerms,
        backupFor: event.backupFor ?? "",
        learnable: event.learnable,
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

export function canUse(review: Review): boolean {
  return review.complete && review.view !== null;
}
