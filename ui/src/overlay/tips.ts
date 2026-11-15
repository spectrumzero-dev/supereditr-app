// The panel's key tips: shown for the first few reviews, until the keys are familiar, unless the
// author closes them sooner or asks in Settings to always see them.

/** How many reviews show the tips. */
export const TIP_REVIEWS = 3;
const SEEN = "tipsSeen";

type Store = Pick<Storage, "getItem" | "setItem">;

/**
 * Whether this review shows the tips. When it does as one of the first few, it is counted;
 * `always` (the setting) shows them without counting.
 */
export function countTip(always: boolean, store?: Store): boolean {
  if (always) return true;
  try {
    store ??= localStorage;
    const seen = Number(store.getItem(SEEN) ?? 0);
    if (seen >= TIP_REVIEWS) return false;
    store.setItem(SEEN, String(seen + 1));
    return true;
  } catch {
    return false;
  }
}

/** The author closed the tips: no later review counts as one of the first few. */
export function closeTips(store?: Store): void {
  try {
    (store ?? localStorage).setItem(SEEN, String(TIP_REVIEWS));
  } catch {
    // Without storage the tips were never shown.
  }
}
