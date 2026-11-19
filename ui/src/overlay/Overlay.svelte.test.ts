import { act, fireEvent, render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DiffView, EngineEvent, SessionPayload } from "../lib/types";
import { ipc } from "./ipc";
import Overlay from "./Overlay.svelte";

type Handlers = Parameters<typeof ipc.listen>[0];

// The panel talks to the app only through `ipc`; the test stands in for the app.
const app = vi.hoisted(() => ({ handlers: null as Handlers | null }));
vi.mock("./ipc", () => ({
  ipc: {
    runPreset: vi.fn(),
    apply: vi.fn(async () => undefined),
    dismiss: vi.fn(),
    copy: vi.fn(async () => undefined),
    resize: vi.fn(),
    focus: vi.fn(),
    typing: vi.fn(async () => undefined),
    openSettings: vi.fn(),
    addWord: vi.fn(async () => true),
    hideTips: vi.fn(async () => undefined),
    listen: (handlers: Handlers) => {
      app.handlers = handlers;
      return () => (app.handlers = null);
    },
  },
}));

// jsdom has no ResizeObserver; the panel uses one only to report its height.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const session: SessionPayload = {
  id: 7,
  text: "Rhyan met brynlee at QuillPoint.",
  appName: "Mail",
  canApply: true,
  auto: false,
  presetId: "proofread",
  presets: [
    { id: "proofread", name: "Proofread" },
    { id: "shorter", name: "Shorter" },
  ],
  showTiming: false,
  alwaysShowTips: false,
  provider: "claude-haiku-5-5",
};
const fixes: DiffView = {
  segments: [
    { type: "equal", text: "Rhyan met " },
    { type: "change", id: 0, old: "brynlee", new: "Brynlee", kind: "capitalization", oldStart: 10 },
    { type: "equal", text: " at QuillPoint." },
  ],
  pending: "",
  changeCount: 1,
};
const done = (view: DiffView): EngineEvent => ({
  type: "done",
  view,
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
});

async function openReview() {
  render(Overlay);
  await act();
  await act(() => app.handlers!.session(session));
}
const engine = (presetId: string, run: number, event: EngineEvent) => act(() => app.handlers!.engine(session.id, presetId, run, event, false));
const replaceKey = () => screen.queryByRole("button", { name: /replace$/ });
const spinner = () => screen.queryByRole("img", { name: "Working" });

describe("the review panel", () => {
  beforeEach(() => vi.clearAllMocks());

  it("shows a spinner where the replace key goes while the engine is answering", async () => {
    await openReview();
    await engine("proofread", 0, { type: "partial", view: fixes, firstTokenMs: 300, source: "claude-haiku-5-5" });
    expect(spinner()).not.toBeNull();
    expect(screen.getByText("claude-haiku-5-5…")).toBeTruthy();
  });

  it("offers the replace key once the result is in", async () => {
    await openReview();
    await engine("proofread", 0, done(fixes));
    expect(spinner()).toBeNull();
    expect(replaceKey()).not.toBeNull();
    expect(screen.getByText("1 change")).toBeTruthy();
  });

  it("offers no replace key when there is nothing to fix", async () => {
    await openReview();
    await engine("proofread", 0, done({ segments: [{ type: "equal", text: session.text }], pending: "", changeCount: 0 }));
    expect(screen.getByText(/nothing to change/)).toBeTruthy();
    expect(replaceKey()).toBeNull();
  });

  it("ignores a late answer from a run another style replaced", async () => {
    await openReview();
    await fireEvent.keyDown(window, { key: "2" });
    expect(ipc.runPreset).toHaveBeenCalledWith(session.id, 1, { presetId: "shorter", steer: null, fresh: false });
    await engine("proofread", 0, done(fixes));
    expect(replaceKey()).toBeNull();
    expect(spinner()).not.toBeNull();
    await engine("shorter", 1, done(fixes));
    expect(replaceKey()).not.toBeNull();
  });

  it("asks the app to close the review on esc", async () => {
    await openReview();
    await fireEvent.keyDown(window, { key: "Escape" });
    expect(ipc.dismiss).toHaveBeenCalledWith(session.id);
    expect(screen.queryByRole("navigation", { name: "Styles" })).toBeNull();
  });

  it("replaces with the fix the author kept on return", async () => {
    await openReview();
    await engine("proofread", 0, done(fixes));
    await fireEvent.keyDown(window, { key: "Enter" });
    expect(ipc.apply).toHaveBeenCalledWith(session.id, "Rhyan met Brynlee at QuillPoint.", false);
  });
});
