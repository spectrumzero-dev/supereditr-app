import { describe, expect, it } from "vitest";
import type { DiffView, EngineEvent } from "../lib/types";
import { canUse, emptyReview, keptWording, notReplacing, offerAnyway, offerToKeep, reduce, runRequest, startReplacing } from "./review";

const view = (text: string): DiffView => ({ segments: [{ type: "equal", text }], pending: "", changeCount: 0 });
const local: EngineEvent = { type: "local", view: view("local"), ms: 3 };
const partial: EngineEvent = { type: "partial", view: view("partial"), firstTokenMs: 300, source: "Apple Intelligence" };
const failed: EngineEvent = { type: "failed", message: "The model stopped at its length limit.", localOnly: true };
const done: EngineEvent = {
  type: "done",
  view: view("done"),
  text: "done",
  source: "claude-haiku-5-5",
  firstTokenMs: 300,
  totalMs: 600,
  changedTokens: [],
  gradeBefore: 12,
  gradeAfter: 8,
  backupFor: null,
  kept: [],
  keptTerms: [],
  learnable: [],
};
const run = (...events: EngineEvent[]) => events.reduce((r, e) => reduce(r, e), emptyReview());

describe("review state", () => {
  it("can use the instant result before the model answers", () => {
    const r = run(local);
    expect(canUse(r)).toBe(true);
    expect(r.view?.segments).toEqual(view("local").segments);
  });

  it("never applies a streaming revision", () => {
    expect(canUse(run(local, partial))).toBe(false);
  });

  it("falls back to the instant result when the model fails mid-stream", () => {
    const r = run(local, partial, failed);
    expect(canUse(r)).toBe(true);
    expect(r.view?.segments).toEqual(view("local").segments);
    expect(r.error).toContain("length limit");
  });

  it("disables apply when a stream fails with no instant result", () => {
    const r = run(partial, { ...failed, localOnly: false });
    expect(canUse(r)).toBe(false);
    expect(r.phase).toBe("failed");
  });

  it("uses the finished revision with one simple timing", () => {
    const r = run(local, partial, done);
    expect(canUse(r)).toBe(true);
    expect(r.timing).toBe("0.6 s");
  });

  it("shows an earlier result again without a time, since no model ran just now", () => {
    const r = reduce(emptyReview(), done, true);
    expect(canUse(r)).toBe(true);
    expect(r.phase).toBe("done");
    expect(r.remembered).toBe(true);
    expect(r.timing).toBe("");
    expect(r.source).toBe("claude-haiku-5-5");
    expect(run(done).remembered).toBe(false);
  });

  it("reports the author's terms a rewrite would drop, and why the backup answered", () => {
    const r = run({ ...done, keptTerms: [{ term: "glanceable", changes: [0] }], backupFor: "Claude Code couldn’t answer: timed out" });
    expect(r.keptTerms).toEqual([{ term: "glanceable", changes: [0] }]);
    expect(r.backupFor).toContain("Claude Code");
  });
});

describe("running a style", () => {
  it("shows a style's earlier result when switching to it, and asks the AI again for R", () => {
    expect(runRequest("polish", undefined, false)).toEqual({ presetId: "polish", steer: null, fresh: false });
    expect(runRequest("polish", undefined, true)).toEqual({ presetId: "polish", steer: null, fresh: true });
  });

  it("sends an instruction trimmed, and a blank one as none", () => {
    expect(runRequest("proofread", "  make it warmer ", false).steer).toBe("make it warmer");
    expect(runRequest("proofread", "   ", false).steer).toBeNull();
  });
});

describe("always keep", () => {
  const named: EngineEvent = {
    ...done,
    view: {
      segments: [
        { type: "change", id: 0, old: "Rhyan", new: "Ryan", kind: "spelling", oldStart: 0 },
        { type: "equal", text: " and " },
        { type: "change", id: 1, old: "their", new: "they're", kind: "grammar", oldStart: 10 },
        { type: "equal", text: " friend " },
        { type: "change", id: 2, old: "Brynlee's", new: "Brynn's", kind: "spelling", oldStart: 24 },
      ],
      pending: "",
      changeCount: 3,
    },
    learnable: ["Rhyan", "Brynlee"],
  };
  const r = run(named);

  it("offers an unknown word only once its change is rejected", () => {
    expect(offerToKeep(r, new Set(), null, new Set())).toBeNull();
    expect(offerToKeep(r, new Set([1]), 1, new Set())).toBeNull();
    expect(offerToKeep(r, new Set([0]), null, new Set())).toBe("Rhyan");
  });

  it("prefers the focused change and skips words already added", () => {
    expect(offerToKeep(r, new Set([0, 2]), 2, new Set())).toBe("Brynlee");
    expect(offerToKeep(r, new Set([0, 2]), 2, new Set(["Brynlee"]))).toBe("Rhyan");
    expect(offerToKeep(run(local), new Set([0]), null, new Set())).toBeNull();
  });
});

describe("kept wording", () => {
  const r = run({
    ...done,
    kept: [0, 1, 2],
    keptTerms: [
      { term: "Rhyan", changes: [0, 2] },
      { term: "wrellly", changes: [1] },
    ],
  });

  it("names a term only while a change that drops it stays rejected", () => {
    expect(keptWording(r, new Set([0, 1, 2]))).toEqual(["Rhyan", "wrellly"]);
    expect(keptWording(r, new Set([0, 2]))).toEqual(["Rhyan"]);
    expect(keptWording(r, new Set([2]))).toEqual(["Rhyan"]);
    expect(keptWording(r, new Set())).toEqual([]);
  });
});

describe("replacing anyway", () => {
  const review = { id: 7, text: "Their going." };

  it("asks for an ordinary replacement first", () => {
    const { state, anyway } = startReplacing(notReplacing<typeof review>(), review);
    expect(anyway).toBe(false);
    expect(state.applying).toBe(review);
  });

  it("brings the review back when the clipboard couldn't be put aside, and the next apply goes ahead anyway", () => {
    const first = startReplacing(notReplacing<typeof review>(), review);
    const offer = offerAnyway(first.state, 7);
    expect(offer?.session).toBe(review);
    expect(offer?.state.applying).toBeNull();
    const second = startReplacing(offer!.state, review);
    expect(second.anyway).toBe(true);
  });

  it("ignores an offer for a review that isn't the one being applied", () => {
    const first = startReplacing(notReplacing<typeof review>(), review);
    expect(offerAnyway(first.state, 8)).toBeNull();
    expect(offerAnyway(notReplacing<typeof review>(), 7)).toBeNull();
  });
});
