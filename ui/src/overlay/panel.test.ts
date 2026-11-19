import { describe, expect, it } from "vitest";
import type { DiffView, EngineEvent, PresetInfo } from "../lib/types";
import { keyAction, viewOf, type Action, type KeyPress, type KeyState, type ViewState } from "./panel";
import { emptyReview, reduce } from "./review";

const presets = (n: number): PresetInfo[] => Array.from({ length: n }, (_, i) => ({ id: `style-${i + 1}`, name: `Style ${i + 1}` }));

const reviewing: KeyState = { notice: false, review: true, steering: false, presets: presets(7), focusedId: null, keepOffer: null };
const key = (name: string, mods: Partial<KeyPress> = {}): KeyPress => ({ key: name, metaKey: false, ctrlKey: false, shiftKey: false, ...mods });
const cmd = (name: string) => key(name, { metaKey: true });
const handled = (action: Action | null) => ({ action, preventDefault: true });

describe("what a key does", () => {
  it("closes a notice with esc or return, marked handled", () => {
    const notice = { ...reviewing, notice: true, review: false };
    expect(keyAction(notice, key("Escape"))).toEqual(handled({ type: "close" }));
    expect(keyAction(notice, key("Enter"))).toEqual(handled({ type: "close" }));
  });

  it("ignores every other key on a notice", () => {
    expect(keyAction({ ...reviewing, notice: true, review: false }, key("r"))).toBeNull();
  });

  it("ignores every key when nothing shows", () => {
    expect(keyAction({ ...reviewing, review: false }, key("Escape"))).toBeNull();
  });

  it("applies on return, with or without a modifier", () => {
    expect(keyAction(reviewing, key("Enter"))).toEqual(handled({ type: "apply" }));
    expect(keyAction(reviewing, cmd("Enter"))).toEqual(handled({ type: "apply" }));
  });

  it("closes the review on esc", () => {
    expect(keyAction(reviewing, key("Escape"))).toEqual(handled({ type: "close" }));
  });

  it("steps forward through the changes with tab and the right and down arrows", () => {
    for (const name of ["Tab", "ArrowRight", "ArrowDown"]) expect(keyAction(reviewing, key(name))).toEqual(handled({ type: "cycle", step: 1 }));
  });

  it("steps back through the changes with shift-tab and the left and up arrows", () => {
    expect(keyAction(reviewing, key("Tab", { shiftKey: true }))).toEqual(handled({ type: "cycle", step: -1 }));
    for (const name of ["ArrowLeft", "ArrowUp"]) expect(keyAction(reviewing, key(name))).toEqual(handled({ type: "cycle", step: -1 }));
  });

  it("switches the focused change with space, backspace or delete", () => {
    for (const name of [" ", "Backspace", "Delete"]) expect(keyAction({ ...reviewing, focusedId: 3 }, key(name))).toEqual(handled({ type: "toggle", id: 3 }));
  });

  it("leaves space alone when no change is focused", () => {
    expect(keyAction(reviewing, key(" "))).toBeNull();
  });

  it("opens the instruction box on slash", () => {
    expect(keyAction(reviewing, key("/"))).toEqual(handled({ type: "startSteering" }));
  });

  it("copies on command-C or control-C", () => {
    expect(keyAction(reviewing, cmd("c"))).toEqual(handled({ type: "copy" }));
    expect(keyAction(reviewing, key("c", { ctrlKey: true }))).toEqual(handled({ type: "copy" }));
  });

  it("does nothing on a plain C", () => {
    expect(keyAction(reviewing, key("c"))).toBeNull();
  });

  it("asks the AI again on R, with or without command", () => {
    expect(keyAction(reviewing, key("r"))).toEqual(handled({ type: "retry" }));
    expect(keyAction(reviewing, cmd("r"))).toEqual(handled({ type: "retry" }));
  });

  // The app takes ⌘C and R from the app under review in either case, so the panel must act on both.
  it("takes a letter the same with caps lock on", () => {
    expect(keyAction(reviewing, cmd("C"))).toEqual(handled({ type: "copy" }));
    expect(keyAction(reviewing, key("R"))).toEqual(handled({ type: "retry" }));
    expect(keyAction({ ...reviewing, keepOffer: "Kyndal" }, key("A"))).toEqual(handled({ type: "keep", word: "Kyndal" }));
  });

  it("leaves a letter typed with Shift alone", () => {
    expect(keyAction(reviewing, key("R", { shiftKey: true }))).toBeNull();
    expect(keyAction(reviewing, key("C", { ctrlKey: true, shiftKey: true }))).toBeNull();
    expect(keyAction({ ...reviewing, keepOffer: "Kyndal" }, key("A", { shiftKey: true }))).toBeNull();
  });

  it("adds the offered word on A", () => {
    expect(keyAction({ ...reviewing, keepOffer: "Kyndal" }, key("a"))).toEqual(handled({ type: "keep", word: "Kyndal" }));
  });

  it("does nothing on A without an offer, or on command-A", () => {
    expect(keyAction(reviewing, key("a"))).toBeNull();
    expect(keyAction({ ...reviewing, keepOffer: "Kyndal" }, cmd("a"))).toBeNull();
  });

  it("opens Settings on command-comma", () => {
    expect(keyAction(reviewing, cmd(","))).toEqual(handled({ type: "openSettings" }));
  });

  it("runs a style on command and its digit, marked handled", () => {
    expect(keyAction(reviewing, cmd("2"))).toEqual(handled({ type: "runPreset", presetId: "style-2" }));
  });

  it("marks command and a digit without a style handled and does nothing", () => {
    expect(keyAction({ ...reviewing, presets: presets(3) }, cmd("5"))).toEqual(handled(null));
  });

  it("runs a style on its digit alone, without marking it handled", () => {
    expect(keyAction(reviewing, key("3"))).toEqual({ action: { type: "runPreset", presetId: "style-3" }, preventDefault: false });
  });

  it("does nothing on a digit alone without a style", () => {
    expect(keyAction({ ...reviewing, presets: presets(3) }, key("5"))).toBeNull();
  });

  it("gives a style past the last chip no key", () => {
    const nine = { ...reviewing, presets: presets(9) };
    expect(viewOf({ ...base, presets: nine.presets }).chips).toHaveLength(7);
    expect(keyAction(nine, cmd("7"))).toEqual(handled({ type: "runPreset", presetId: "style-7" }));
    expect(keyAction(nine, cmd("8"))).toBeNull();
    expect(keyAction(nine, key("9"))).toBeNull();
  });

  it("does nothing on zero", () => {
    expect(keyAction(reviewing, cmd("0"))).toBeNull();
  });
});

describe("what a key does while an instruction is typed", () => {
  const steering = { ...reviewing, steering: true };

  it("drops the instruction on esc", () => {
    expect(keyAction(steering, key("Escape"))).toEqual(handled({ type: "cancelSteering" }));
  });

  it("runs the instruction on return", () => {
    expect(keyAction(steering, key("Enter"))).toEqual(handled({ type: "submitSteering" }));
  });

  it("types a character or backspace the keyboard tap took from the app", () => {
    expect(keyAction(steering, key("Q"), true)).toEqual({ action: { type: "typeSteering", key: "Q" }, preventDefault: false });
    expect(keyAction(steering, key("Backspace"), true)).toEqual({ action: { type: "typeSteering", key: "Backspace" }, preventDefault: false });
  });

  it("leaves keys typed into the instruction box to the box", () => {
    expect(keyAction(steering, key("Q"))).toBeNull();
  });

  it("ignores review keys and shortcuts while typing", () => {
    expect(keyAction(steering, key("Tab"), true)).toBeNull();
    expect(keyAction(steering, cmd("c"), true)).toBeNull();
    expect(keyAction(steering, cmd("1"))).toBeNull();
  });
});

const view = (text: string): DiffView => ({ segments: [{ type: "equal", text }], pending: "", changeCount: 0 });
const fixes: DiffView = {
  segments: [
    { type: "change", id: 0, old: "Rhyan", new: "Ryan", kind: "spelling", oldStart: 0 },
    { type: "equal", text: " met " },
    { type: "change", id: 1, old: "brynlee", new: "Brynlee", kind: "capitalization", oldStart: 10, note: "A name starts with a capital." },
  ],
  pending: "",
  changeCount: 2,
};
const done: EngineEvent = {
  type: "done",
  view: fixes,
  text: "",
  source: "claude-haiku-5-5",
  firstTokenMs: 300,
  totalMs: 600,
  changedTokens: [],
  gradeBefore: null,
  gradeAfter: null,
  backupFor: null,
  kept: [],
  keptTerms: [],
  learnable: [],
};
const run = (...events: EngineEvent[]) => events.reduce((r, e) => reduce(r, e), emptyReview());

const base: ViewState = {
  review: run(done),
  canApply: true,
  showTiming: false,
  presets: presets(3),
  presetId: "style-1",
  rejected: new Set(),
  focusedId: null,
  added: new Set(),
  keyboard: true,
  showTips: false,
  anyway: false,
};

describe("what the panel shows", () => {
  it("lists a chip for each style, numbered, with the current one active", () => {
    expect(viewOf({ ...base, presetId: "style-2" }).chips).toEqual([
      { id: "style-1", name: "Style 1", key: 1, active: false },
      { id: "style-2", name: "Style 2", key: 2, active: true },
      { id: "style-3", name: "Style 3", key: 3, active: false },
    ]);
  });

  it("shows a spinner and Checking until the engine answers", () => {
    const panel = viewOf({ ...base, review: emptyReview() });
    expect(panel.busy).toBe(true);
    expect(panel.action).toBe("working");
    expect(panel.status).toMatchObject({ dot: "pulsing", text: "Checking…" });
  });

  it("names the model while its answer streams in", () => {
    const panel = viewOf({ ...base, review: run({ type: "partial", view: fixes, firstTokenMs: 300, source: "claude-haiku-5-5" }) });
    expect(panel.status).toMatchObject({ dot: "pulsing", text: "claude-haiku-5-5…", source: "" });
    expect(panel.action).toBe("working");
  });

  it("offers the replace key and counts the changes once the answer is in", () => {
    const panel = viewOf(base);
    expect(panel.action).toBe("replace");
    expect(panel.usable).toBe(true);
    expect(panel.status).toMatchObject({ dot: "good", text: "2 changes", source: "claude-haiku-5-5" });
  });

  it("offers the copy key and says so where text can't be replaced", () => {
    const panel = viewOf({ ...base, canApply: false });
    expect(panel.action).toBe("copy");
    expect(panel.copyOnly).toBe(true);
  });

  it("offers no key and says all is well when there is nothing to fix", () => {
    const panel = viewOf({ ...base, review: run({ ...done, view: view("Kyndal joined QuillPoint.") }) });
    expect(panel.unchanged).toBe(true);
    expect(panel.action).toBe("none");
    expect(panel.status.text).toBe("No changes");
  });

  it("doesn't say text can't be replaced when there is nothing to fix", () => {
    expect(viewOf({ ...base, canApply: false, review: run({ ...done, view: view("Fine.") }) }).copyOnly).toBe(false);
  });

  it("says it couldn't finish when the engine failed with nothing to fall back to", () => {
    const panel = viewOf({ ...base, review: run({ type: "failed", message: "No answer.", localOnly: false }) });
    expect(panel.status).toMatchObject({ dot: "bad", text: "Couldn’t finish" });
    expect(panel.action).toBe("none");
  });

  it("shows the time only when the settings ask for it", () => {
    expect(viewOf(base).status.timing).toBe("");
    expect(viewOf({ ...base, showTiming: true }).status.timing).toBe("0.6 s");
  });

  it("marks a result shown again from earlier", () => {
    const panel = viewOf({ ...base, review: reduce(emptyReview(), done, true) });
    expect(panel.status.fromEarlier).toBe(true);
    expect(panel.status.title).toContain("from earlier");
  });

  it("shortens a backup source's name in the footer", () => {
    expect(viewOf({ ...base, review: run({ ...done, source: "Apple Intelligence (backup)" }) }).status.source).toBe("Apple Intelligence");
  });

  it("names the next and switch keys only with more than one change and a focused change", () => {
    expect(viewOf(base).keys).toEqual({ next: true, toggle: null, instruct: true });
    expect(viewOf({ ...base, focusedId: 0 }).keys.toggle).toBe("keep original");
    expect(viewOf({ ...base, focusedId: 0, rejected: new Set([0]) }).keys.toggle).toBe("restore");
  });

  it("names no review keys when the panel doesn't take the keyboard", () => {
    expect(viewOf({ ...base, keyboard: false, focusedId: 0 }).keys).toEqual({ next: false, toggle: null, instruct: false });
  });

  it("explains the focused change with its note, or what it changes", () => {
    expect(viewOf({ ...base, focusedId: 1 }).note).toEqual({ label: "Capitalization", text: "A name starts with a capital." });
    expect(viewOf({ ...base, focusedId: 0 }).note?.text).toBe("“Rhyan” → “Ryan”");
    expect(viewOf({ ...base, focusedId: 0, rejected: new Set([0]) }).note?.text).toBe("Keeping your original.");
    expect(viewOf(base).note).toBeNull();
  });

  it("offers to always keep an unknown word the author kept", () => {
    const review = run({ ...done, learnable: ["Rhyan"] });
    expect(viewOf({ ...base, review }).keepOffer).toBeNull();
    expect(viewOf({ ...base, review, rejected: new Set([0]) }).keepOffer).toBe("Rhyan");
  });

  it("shows the key tips only for a finished review with changes, while the panel has the keyboard", () => {
    expect(viewOf({ ...base, showTips: true }).banner).toBe("tips");
    expect(viewOf({ ...base, showTips: true, keyboard: false }).banner).toBeNull();
    expect(viewOf({ ...base, showTips: true, review: emptyReview() }).banner).toBeNull();
  });

  it("shows the replace-anyway warning in place of the tips", () => {
    expect(viewOf({ ...base, showTips: true, anyway: true }).banner).toBe("anyway");
  });
});
