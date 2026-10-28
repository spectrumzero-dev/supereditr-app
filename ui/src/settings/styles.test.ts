import { describe, expect, it } from "vitest";
import type { Preset } from "../lib/types";
import { describeLosses, isDefault, losesWork, resetLosses } from "./styles";

const p = (id: string, name = id, instruction = `do ${id}`): Preset => ({ id, name, instruction, builtin: true, shortcut: null });
const defaults = [p("proofread", "Proofread"), p("polish", "Polish"), p("shorter", "Shorter")];

describe("resetLosses", () => {
  it("finds nothing to lose in the defaults", () => {
    expect(losesWork(resetLosses(defaults, defaults))).toBe(false);
    expect(isDefault(defaults, defaults)).toBe(true);
  });

  it("doesn't count deleted built-ins as a loss, but there is still something to reset", () => {
    const current = [defaults[0], defaults[2]];
    expect(losesWork(resetLosses(current, defaults))).toBe(false);
    expect(isDefault(current, defaults)).toBe(false);
  });

  it("names edited built-ins, added styles, and a new order", () => {
    const current = [{ ...defaults[1], instruction: "mine" }, defaults[0], { ...p("custom-1", "Legal"), builtin: false }];
    const losses = resetLosses(current, defaults);
    expect(losses).toEqual({ edited: ["Polish"], custom: ["Legal"], reordered: true });
    expect(describeLosses(losses)).toBe("your changes to “Polish”, the style you added (“Legal”), and your order");
  });

  it("counts a shortcut as an edit", () => {
    const current = [{ ...defaults[0], shortcut: "Control+Alt+P" }, defaults[1], defaults[2]];
    expect(describeLosses(resetLosses(current, defaults))).toBe("your changes to “Proofread”");
  });
});
