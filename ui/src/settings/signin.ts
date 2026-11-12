import type { ProviderCheck } from "../lib/types";
import { words } from "../lib/platform";

/** The words for a check that failed: Settings' own for a problem with a code, else the app's. */
export function problemText(check: ProviderCheck): string {
  if (check.code === "claude_sign_in") return words().claudeSignIn;
  return check.problem ?? "";
}

export type SignInEnd = "signed-in" | "cancelled" | "timed-out" | "hidden";

export interface WaitOptions {
  /** How often to ask, in milliseconds. */
  every: number;
  /** How long to keep asking, in milliseconds. */
  limit: number;
  /** Whether Settings is still on screen: nobody waits on a hidden window. */
  visible: () => boolean;
  cancelled: () => boolean;
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
}

/** Asks `signedIn` every `every` ms until it says yes, the user cancels, Settings goes out of sight,
 * or `limit` passes. */
export async function waitForSignIn(signedIn: () => Promise<boolean>, options: WaitOptions): Promise<SignInEnd> {
  const sleep = options.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
  const now = options.now ?? (() => Date.now());
  const start = now();
  for (;;) {
    await sleep(options.every);
    if (options.cancelled()) return "cancelled";
    if (!options.visible()) return "hidden";
    if (await signedIn().catch(() => false)) return "signed-in";
    if (options.cancelled()) return "cancelled";
    if (now() - start >= options.limit) return "timed-out";
  }
}
