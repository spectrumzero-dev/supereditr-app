// Browser-only preview with canned data, used for design work outside the native app.
import type { DiffView, EngineEvent, SessionPayload, StatusPayload } from "../lib/types";

const text = "Their going to love this, it work good and its really fast. Lets ship it tommorow.";
const local: DiffView = {
  segments: [
    { type: "change", id: 0, old: "Their", new: "They're", kind: "grammar", oldStart: 0, note: "Did you mean the contraction “they're”?" },
    { type: "equal", text: " going to love this, it work good and its really fast. Lets ship it " },
    { type: "change", id: 1, old: "tommorow", new: "tomorrow", kind: "spelling", oldStart: 72, note: "Possible spelling mistake." },
    { type: "equal", text: "." },
  ],
  pending: "",
  changeCount: 2,
};
const done: DiffView = {
  segments: [
    { type: "change", id: 0, old: "Their", new: "They're", kind: "grammar", oldStart: 0 },
    { type: "equal", text: " going to love this" },
    { type: "change", id: 1, old: ", it work good", new: ". It works well,", kind: "style", oldStart: 18 },
    { type: "equal", text: " and " },
    { type: "change", id: 2, old: "its", new: "it's", kind: "punctuation", oldStart: 37 },
    { type: "equal", text: " really fast. " },
    { type: "change", id: 3, old: "Lets", new: "Let's", kind: "punctuation", oldStart: 55 },
    { type: "equal", text: " ship it " },
    { type: "change", id: 4, old: "tommorow", new: "tomorrow", kind: "spelling", oldStart: 69 },
    { type: "equal", text: "." },
  ],
  pending: "",
  changeCount: 5,
};

type Handlers = {
  session: (payload: SessionPayload) => void;
  engine: (id: number, presetId: string, event: EngineEvent) => void;
  status: (payload: StatusPayload) => void;
};

let handlers: Handlers | null = null;

function play(presetId: string) {
  const h = handlers;
  if (!h) return;
  setTimeout(() => h.engine(1, presetId, { type: "local", view: local, ms: 4 }), 30);
  setTimeout(() => h.engine(1, presetId, { type: "partial", view: { ...done, segments: done.segments.slice(0, 4), pending: " really fast. Lets ship it tommorow." }, firstTokenMs: 290, source: "claude-haiku-5-5" }), 350);
  setTimeout(() => h.engine(1, presetId, { type: "done", view: done, text: "", source: "claude-haiku-5-5", firstTokenMs: 290, totalMs: 520, changedTokens: [], gradeBefore: null, gradeAfter: null, backupFor: null, kept: [], keptTerms: [], learnable: [] }), 650);
}

export const demo = {
  invoke(cmd: string, args?: Record<string, unknown>) {
    if (cmd === "run_preset") play(String(args?.presetId));
    if (cmd === "add_word") return true;
    return undefined;
  },
  listen(h: Handlers) {
    handlers = h;
    const notice = new URLSearchParams(location.search).get("notice");
    setTimeout(() => {
      if (notice) {
        h.status({ id: 1, kind: "notice", message: notice });
        return;
      }
      h.session({
        id: 1,
        text,
        appName: "Mail",
        canApply: true,
        auto: new URLSearchParams(location.search).has("auto"),
        presetId: "proofread",
        presets: ["Proofread", "Polish", "Shorter", "Friendlier", "Professional", "Simplify", "To English"].map((name, i) => ({
          id: ["proofread", "polish", "shorter", "friendly", "professional", "simplify", "english"][i],
          name,
        })),
        showTiming: true,
        provider: "claude-haiku-5-5",
      });
      play("proofread");
    }, 50);
    return () => (handlers = null);
  },
};
