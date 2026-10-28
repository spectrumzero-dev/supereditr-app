// Keeps Apple Intelligence's status current while a settings page is open. It changes outside
// SuperEditr: the user turns it on in System Settings, or its model finishes downloading.
import type { BuiltinAi } from "../lib/types";

export function watchBuiltinAi(
  fetch: () => Promise<BuiltinAi>,
  onstatus: (status: BuiltinAi) => void,
  target: EventTarget = window,
  intervalMs = 1500,
): () => void {
  let last: BuiltinAi | null = null;
  let stopped = false;
  const refresh = async () => {
    try {
      const status = await fetch();
      if (stopped) return;
      last = status;
      onstatus(status);
    } catch {
      // Keep the last known status; the next focus or tick tries again.
    }
  };
  refresh();
  // Coming back from System Settings focuses the window; a finishing download doesn't, so poll
  // while it's unavailable on a Mac that supports it.
  target.addEventListener("focus", refresh);
  const timer = setInterval(() => {
    if (last?.supported && !last.available) refresh();
  }, intervalMs);
  return () => {
    stopped = true;
    target.removeEventListener("focus", refresh);
    clearInterval(timer);
  };
}
