import { describe, expect, it } from "vitest";
import { applyView, changeLabel, changes, keySymbols, sourceLabel, summarize } from "./diff";
import type { DiffView } from "./types";

const view: DiffView = {
  segments: [
    { type: "equal", text: "This " },
    { type: "change", id: 0, old: "are", new: "is", kind: "grammar", oldStart: 5 },
    { type: "equal", text: " " },
    { type: "change", id: 1, old: "teh", new: "the", kind: "spelling", oldStart: 9 },
    { type: "equal", text: " draft." },
  ],
  pending: "",
  changeCount: 2,
};

describe("applyView", () => {
  it("matches the Rust implementation", () => {
    expect(applyView(view, new Set())).toBe("This is the draft.");
    expect(applyView(view, new Set([1]))).toBe("This is teh draft.");
    expect(applyView({ ...view, pending: " More." }, new Set([0, 1]))).toBe("This are teh draft. More.");
  });
});

describe("summaries and keys", () => {
  it("summarizes change kinds", () => {
    expect(summarize(view)).toBe("1 grammar, 1 spelling");
  });
  it("names the offline checker's fixes on top of an AI's", () => {
    const checked: DiffView = {
      ...view,
      segments: view.segments.map((s) => (s.type === "change" && s.id === 1 ? { ...s, ruleBased: true } : s)),
    };
    const [ai, rule] = changes(checked);
    expect(changeLabel(ai)).toBe("Grammar");
    expect(changeLabel(rule)).toBe("Spelling · Rule-based checker");
    expect(sourceLabel("gemma3:4b", checked)).toBe("gemma3:4b + rule-based checker");
    expect(sourceLabel("gemma3:4b", view)).toBe("gemma3:4b");
  });
  it("formats accelerators", () => {
    expect(keySymbols("Control+Alt+E")).toBe("⌃⌥E");
    expect(keySymbols("Control+Alt+Shift+E")).toBe("⌃⌥⇧E");
    expect(keySymbols("Control+Alt+E", false)).toBe("Ctrl+Alt+E");
    expect(keySymbols("Command+E Command+E")).toBe("⌘E E");
  });
});
