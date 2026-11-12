// Whether an AI is there to review. Without one, the rule-based checker reviews, and it handles
// only Proofread on English text: every other style, a typed instruction and other languages need
// an AI (see `run` in crates/core/src/engine.rs).
import type { BuiltinAi, ProviderCheck } from "../lib/types";

/** What Settings knows about the AI that reviews:
 * - "checking": the chosen AI's check or Apple Intelligence's status hasn't come back yet;
 * - "ready": the chosen AI works, or Apple Intelligence is on (it also answers when a chosen AI fails);
 * - "off": neither, and Apple Intelligence could review once it's turned on;
 * - "none": neither, and nothing built in can review. */
export type AiState = "checking" | "ready" | "off" | "none";

/** `chosen` is whether an AI is chosen in Models, and `check` its check, once it's back. */
export function aiState(chosen: boolean, check: ProviderCheck | null, builtin: BuiltinAi | null): AiState {
  if ((chosen && check?.ready) || builtin?.available) return "ready";
  if ((chosen && !check) || !builtin) return "checking";
  return builtin.supported ? "off" : "none";
}

/** Whether a review in this style needs an AI, whatever the text. */
export const needsAi = (presetId: string) => presetId !== "proofread";
