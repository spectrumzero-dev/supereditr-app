// Which saved AI a Models row shows. One row per kind, but earlier versions allowed several AIs of
// a kind (two servers, two keys): the row shows the one the user picked, else the one in use, else
// the first, so the AI in use is never hidden behind another of its kind.
import type { ProviderKind, ProviderProfile } from "../lib/types";

export function profileFor(
  providers: ProviderProfile[],
  kind: ProviderKind,
  picked: string | undefined,
  active: string | null,
): ProviderProfile | null {
  const all = providers.filter((p) => p.kind === kind);
  return all.find((p) => p.id === picked) ?? all.find((p) => p.id === active) ?? all[0] ?? null;
}

/** What a row's setup offers for the saved AI it shows: check the one in use again, or make the
 * shown one (possibly a second account of the same kind) the one in use. */
export function setupAction(shown: ProviderProfile, active: string | null): "check" | "use" {
  return shown.id === active ? "check" : "use";
}

/** Whether a row shows the Early access label: an AI reached with the user's own key or account,
 * or an OpenAI-compatible server that isn't on this Mac (OpenRouter, Groq). A server on this Mac,
 * such as LM Studio, and an address that isn't complete yet, show none. */
export function earlyAccess(kind: ProviderKind, shown: ProviderProfile | null): boolean {
  if (kind !== "open_ai_compatible") return kind === "anthropic" || kind === "open_ai" || kind === "gemini" || kind === "claude_cli";
  if (!shown) return false;
  try {
    const host = new URL(shown.base_url).hostname;
    return !(host === "localhost" || host === "[::1]" || host.startsWith("127."));
  } catch {
    return false;
  }
}
