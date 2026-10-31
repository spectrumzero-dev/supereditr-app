import { describe, expect, it } from "vitest";
import { mergeUnsaved } from "./merge";

const base = { style_notes: "", auto_popup: true, excluded_apps: ["com.1password.1password"], words: ["Rhyan"] };

describe("mergeUnsaved", () => {
  it("keeps an unsaved edit when the menu bar turns SuperEditr off in an app", () => {
    const unsaved = { ...base, style_notes: "Short sentences." };
    const fresh = { ...base, excluded_apps: ["com.1password.1password", "com.apple.TextEdit"] };
    expect(mergeUnsaved(base, unsaved, fresh)).toEqual({
      ...base,
      style_notes: "Short sentences.",
      excluded_apps: ["com.1password.1password", "com.apple.TextEdit"],
    });
  });

  it("takes every change from the app when nothing is waiting to be saved", () => {
    const fresh = { ...base, words: ["Rhyan", "Brynlee"], auto_popup: false };
    expect(mergeUnsaved(base, { ...base }, fresh)).toEqual(fresh);
  });

  it("lets the user's own edit win a field both changed", () => {
    const unsaved = { ...base, excluded_apps: [] };
    const fresh = { ...base, excluded_apps: ["com.1password.1password", "com.apple.TextEdit"] };
    expect(mergeUnsaved(base, unsaved, fresh).excluded_apps).toEqual([]);
  });
});
