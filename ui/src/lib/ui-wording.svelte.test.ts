// tests/contracts/ui-wording.json lists the words in SuperEditr's windows that the end-to-end UI
// drivers look for. This renders the components that show the `ui` entries, on each platform whose
// driver looks, and checks that they show the fixture's text. src-tauri/src/ui_wording_contract.rs
// checks that each driver's pattern matches that text and is still in the driver.
import { act, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Overlay from "../overlay/Overlay.svelte";
import App from "../settings/App.svelte";
import ClaudeSignIn from "../settings/ClaudeSignIn.svelte";
import { problemText } from "../settings/signin";
import Update from "../update/Update.svelte";
import { setPlatform, type Platform } from "./platform";
import type { BuiltinAi, DiffView, EngineEvent, SessionPayload, UpdateStatus } from "./types";

interface Entry {
  id: string;
  made_by: "ui" | "rust" | "source";
  file: string;
  role?: string;
  text: string | Partial<Record<Platform, string>>;
  matched_by: { driver: Platform }[];
}

// Read from disk: the component tests' server serves only ui/'s own files, so the `?raw` import
// contracts.test.ts uses is refused here. The UI's types leave out Node's, hence the casts.
const { readFileSync } = (await import("node:fs" as string)) as { readFileSync: (path: string, encoding: "utf8") => string };
const here = (import.meta as ImportMeta & { dirname: string }).dirname;
const wording: Entry[] = JSON.parse(readFileSync(`${here}/../../../tests/contracts/ui-wording.json`, "utf8")).wording;

// The review panel talks to the app only through `ipc`; the test stands in for the app.
type Handlers = Parameters<typeof import("../overlay/ipc").ipc.listen>[0];
const panelApp = vi.hoisted(() => ({ handlers: null as Handlers | null }));
vi.mock("../overlay/ipc", () => ({
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
      panelApp.handlers = handlers;
      return () => (panelApp.handlers = null);
    },
  },
}));

// Settings runs on its browser preview's answers, with these in place of the app's: a first run with
// no AI chosen and Accessibility allowed, on the platform under test.
const settingsApp = vi.hoisted(() => ({
  platform: "macos" as string,
  builtin: { supported: false, available: false, message: null } as BuiltinAi,
  scanForever: false,
}));
vi.mock("../settings/ipc", async (importOriginal) => {
  const real = await importOriginal<typeof import("../settings/ipc")>();
  return {
    ...real,
    api: {
      ...real.api,
      get: async () => {
        const payload = await real.api.get();
        const config = { ...structuredClone(payload.config), active_provider: null, onboarded: false };
        return { ...payload, config, platform: settingsApp.platform, accessibility: true };
      },
      builtinAi: async () => settingsApp.builtin,
      scan: () => (settingsApp.scanForever ? new Promise<never>(() => {}) : real.api.scan()),
    },
  };
});

// The update window reads its status from the app, then listens for changes.
const updateApp = vi.hoisted(() => ({ status: { state: "checking" } as UpdateStatus }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async (cmd: string) => (cmd === "update_status" ? updateApp.status : undefined)) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

// jsdom has no ResizeObserver; the panel and Get Started use one only to report their height.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
// Nor Web Animations, which Svelte's transitions use: the panel's confirmation row slides in. Here
// every animation finishes at once.
Element.prototype.animate ??= function () {
  const animation = { onfinish: null as (() => void) | null, cancel() {}, finish() {}, currentTime: 0 };
  queueMicrotask(() => animation.onfinish?.());
  return animation as unknown as Animation;
};

const session: SessionPayload = {
  id: 7,
  text: "Their going to love this.",
  appName: "Notepad",
  canApply: true,
  auto: false,
  presetId: "proofread",
  presets: [
    { id: "proofread", name: "Proofread" },
    { id: "shorter", name: "Shorter" },
  ],
  showTiming: false,
  alwaysShowTips: false,
  provider: null,
};
const oneFix: DiffView = {
  segments: [
    { type: "change", id: 0, old: "Their", new: "They're", kind: "grammar", oldStart: 0 },
    { type: "equal", text: " going to love this." },
  ],
  pending: "",
  changeCount: 1,
};
const noFix: DiffView = { segments: [{ type: "equal", text: session.text }], pending: "", changeCount: 0 };
const done = (view: DiffView): EngineEvent => ({
  type: "done",
  view,
  text: "",
  source: "Harper",
  firstTokenMs: 30,
  totalMs: 60,
  changedTokens: [],
  gradeBefore: null,
  gradeAfter: null,
  backupFor: null,
  kept: [],
  keptTerms: [],
  learnable: [],
});

/** A review of `session` with `changes` in, after `overrides` to the session. */
async function review(event: EngineEvent | null, overrides: Partial<SessionPayload> = {}) {
  render(Overlay);
  await act();
  await act(() => panelApp.handlers!.session({ ...session, ...overrides }));
  if (event) await act(() => panelApp.handlers!.engine(session.id, session.presetId, 0, event, false));
}

const key = (k: string, mods: Partial<Pick<KeyboardEvent, "altKey" | "metaKey" | "ctrlKey">> = {}) => fireEvent.keyDown(window, { key: k, ...mods });

/** Settings on its first page, then on the page named `page`. */
async function settings(platform: Platform, page?: string) {
  settingsApp.platform = platform;
  render(App);
  const nav = await screen.findByRole("button", { name: page ?? "Get Started" });
  if (page) await fireEvent.click(nav);
}

async function update(status: UpdateStatus) {
  updateApp.status = status;
  render(Update);
  await act();
}

/** Puts on screen what shows the entry `id` on `platform`, or returns the text where a function
 * makes it. */
async function show(id: string, platform: Platform): Promise<string | void> {
  const copyKey = platform === "macos" ? { metaKey: true } : { ctrlKey: true };
  switch (id) {
    case "change-count":
    case "replace-key":
      return review(done(oneFix));
    case "no-changes":
      return review(done(noFix));
    case "working":
      return review({ type: "partial", view: oneFix, firstTokenMs: 30, source: "Harper" });
    case "key-tips":
    case "hide-tips":
      return review(done(oneFix), { alwaysShowTips: true });
    case "tips-hidden":
      await review(done(oneFix), { alwaysShowTips: true });
      return void (await fireEvent.click(screen.getByRole("button", { name: "Don’t show tips again" })));
    case "copy-only":
      return review(done(oneFix), { canApply: false });
    case "nothing-replaced":
      await review(done(oneFix), { canApply: false });
      return void (await key("Enter", { altKey: true }));
    case "copied":
      await review(done(oneFix));
      return void (await key("c", copyKey));
    case "change-described":
      await review(done(oneFix));
      return void (await key("Tab"));
    case "copy-suggestion":
      render(Overlay);
      await act();
      return act(() =>
        panelApp.handlers!.status({ id: 7, kind: "error", message: "SuperEditr couldn’t confirm the change.", copy: "They're going to love this." }),
      );

    case "update-checking":
      return update({ state: "checking" });
    case "update-up-to-date":
      return update({ state: "up-to-date", version: "0.1.9" });
    case "update-check-failed":
    case "update-try-again":
      return update({ state: "check-failed" });
    case "update-install-failed":
    case "update-close":
      return update({ state: "install-failed", version: "99.0.0" });
    case "update-downloading":
      return update({ state: "downloading", version: "99.0.0", percent: 40 });
    case "update-ready":
    case "update-later":
    case "update-restart-now":
      return update({ state: "ready", version: "99.0.0" });

    case "nav-get-started":
    case "nav-shortcuts":
    case "nav-general":
    case "nav-privacy":
    case "nav-styles":
    case "find-accounts":
    case "no-ai":
    case "done":
    case "try-it-review":
    case "set-up-omarchy":
      return settings(platform);
    case "review-shortcut":
      return settings(platform, "Shortcuts");
    case "open-at-login":
      return settings(platform, "General");
    case "password-fields-never-read":
    case "add-app":
      return settings(platform, "Privacy");
    case "scan":
    case "early-access":
      return settings(platform, "Models");
    case "scanning":
      settingsApp.scanForever = true;
      await settings(platform, "Models");
      return void (await fireEvent.click(screen.getByRole("button", { name: "Scan" })));
    case "scan-again":
    case "use-found-account":
      await settings(platform, "Models");
      return void (await fireEvent.click(screen.getByRole("button", { name: "Scan" })));
    case "set-up-ai":
    case "styles-need-ai":
      return settings(platform, "Styles");
    case "turn-on-apple-intelligence":
      settingsApp.builtin = { supported: true, available: false, message: "Apple Intelligence is off." };
      return settings(platform, "Styles");
    case "sign-in-to-claude":
      render(ClaudeSignIn, { profileId: "claude_cli", onsignedin: () => {} });
      return;
    case "claude-not-signed-in":
      return problemText({ ready: false, problem: "Claude Code isn't signed in.", code: "claude_sign_in", models: [] });
    default:
      throw new Error(`tests/contracts/ui-wording.json lists ${id}, which this test doesn't show`);
  }
}

const pageText = () => (document.body.textContent ?? "").replace(/\s+/g, " ");

describe("ui-wording.json", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    settingsApp.builtin = { supported: false, available: false, message: null };
    settingsApp.scanForever = false;
  });
  afterEach(() => setPlatform("macos"));

  it("lists the words the UI shows", () => expect(wording.filter((e) => e.made_by === "ui").length).toBeGreaterThan(0));

  for (const entry of wording.filter((e) => e.made_by === "ui")) {
    const platforms = [...new Set(entry.matched_by.map((m) => m.driver))];
    for (const platform of platforms) {
      const text = typeof entry.text === "string" ? entry.text : entry.text[platform];
      it(`${entry.id} on ${platform}: ${entry.file} shows ${JSON.stringify(text)}`, async () => {
        expect(text, `no text for ${platform}`).toBeTypeOf("string");
        setPlatform(platform);
        const made = await show(entry.id, platform);
        if (typeof made === "string") return expect(made).toBe(text);
        await vi.waitFor(() => {
          if (entry.role) {
            const named = screen.queryAllByRole(entry.role, { name: text });
            expect(named.length, `no ${entry.role} named ${JSON.stringify(text)} in: ${pageText()}`).toBeGreaterThan(0);
          } else {
            expect(pageText()).toContain(text);
          }
        });
      });
    }
  }
});
