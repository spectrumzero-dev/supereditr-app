import { describe, expect, it } from "vitest";
import type { ProviderProfile } from "../lib/types";
import { mergeUnsaved, saveUntilCurrent } from "./merge";

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

  it("keeps a deliberate exclusion removal alongside another writer's addition", () => {
    const unsaved = { ...base, excluded_apps: [] };
    const fresh = { ...base, excluded_apps: ["com.1password.1password", "com.apple.TextEdit"] };
    expect(mergeUnsaved(base, unsaved, fresh).excluded_apps).toEqual(["com.apple.TextEdit"]);
  });

  it("merges dictionary changes and matches entries using the dictionary's identity", () => {
    const original = { words: ["  İona ", "QuillPoint", "Kyndal"] };
    const unsaved = { words: ["QuillPoint", "kyndal", "Brynlee"] };
    const fresh = { words: ["iona", "Kyndal", "brynlee", "Rhyan"] };
    expect(mergeUnsaved(original, unsaved, fresh).words).toEqual(["Kyndal", "brynlee", "Rhyan"]);
    expect(mergeUnsaved(base, { ...base, words: ["Rhyan", "Brynlee"] }, { ...base, words: ["Rhyan", "Kyndal"] }).words)
      .toEqual(["Rhyan", "Kyndal", "Brynlee"]);
  });

  it("does not restore an exclusion removed by another writer", () => {
    const original = { excluded_apps: ["com.example.QuillPoint"] };
    const unsaved = { excluded_apps: ["com.example.quillpoint", "com.example.Brynlee"] };
    const fresh = { excluded_apps: ["com.example.brynlee", "com.example.Rhyan"] };
    expect(mergeUnsaved(original, unsaved, fresh).excluded_apps).toEqual(fresh.excluded_apps);
  });

  it("merges providers by ID, respecting removals and disjoint edits to a profile", () => {
    const provider = (id: string): ProviderProfile => ({
      id, kind: "ollama", label: id, base_url: "http://127.0.0.1:11434", model: "original",
    });
    const kept = provider("kept");
    const removedHere = provider("removed-here");
    const removedElsewhere = provider("removed-elsewhere");
    const original = { providers: [kept, removedHere, removedElsewhere], active_provider: null as string | null };
    const unsaved = {
      providers: [{ ...kept, model: "new-model" }, { ...removedElsewhere, model: "discarded" }, provider("local")],
      active_provider: "removed-elsewhere",
    };
    const fresh = {
      providers: [{ ...kept, base_url: "http://127.0.0.1:11435" }, removedHere, provider("other")],
      active_provider: null,
    };
    expect(mergeUnsaved(original, unsaved, fresh)).toEqual({
      providers: [{ ...kept, model: "new-model", base_url: "http://127.0.0.1:11435" }, provider("other"), provider("local")],
      active_provider: null,
    });
  });
});

describe("saveUntilCurrent", () => {
  type Settings = typeof base;

  /** The app's settings and revision, and Settings' copy, as App.svelte keeps them. */
  function setUp() {
    const app = { config: base as Settings, revision: 1 };
    const copy = { base: base as Settings, revision: 1, unsaved: base as Settings };
    const save = async () => {
      if (copy.revision !== app.revision) return false;
      app.config = copy.unsaved;
      app.revision += 1;
      copy.base = copy.unsaved;
      copy.revision = app.revision;
      return true;
    };
    const reload = async () => {
      copy.unsaved = mergeUnsaved(copy.base, copy.unsaved, app.config);
      copy.base = app.config;
      copy.revision = app.revision;
    };
    return { app, copy, save, reload };
  }

  it("redoes a Settings save the app refused after the menu bar turned an app off, keeping both", async () => {
    const { app, copy, save, reload } = setUp();
    copy.unsaved = { ...copy.unsaved, style_notes: "Short sentences." };
    // The menu bar saves while Settings' save, based on revision 1, is on its way.
    app.config = { ...app.config, excluded_apps: [...app.config.excluded_apps, "com.apple.TextEdit"] };
    app.revision += 1;
    expect(await saveUntilCurrent(save, reload)).toBe(true);
    expect(app.config).toEqual({ ...base, style_notes: "Short sentences.", excluded_apps: ["com.1password.1password", "com.apple.TextEdit"] });
  });

  it("stops after three refusals, leaving the changes unsaved", async () => {
    const { app, copy, reload } = setUp();
    copy.unsaved = { ...copy.unsaved, style_notes: "Short sentences." };
    let tries = 0;
    const refused = async () => {
      tries += 1;
      return false;
    };
    expect(await saveUntilCurrent(refused, reload)).toBe(false);
    expect(tries).toBe(3);
    expect(app.config.style_notes).toBe("");
    expect(copy.unsaved.style_notes).toBe("Short sentences.");
  });
});
