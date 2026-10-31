import { describe, expect, it } from "vitest";
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

  it("lets the user's own edit win a field both changed", () => {
    const unsaved = { ...base, excluded_apps: [] };
    const fresh = { ...base, excluded_apps: ["com.1password.1password", "com.apple.TextEdit"] };
    expect(mergeUnsaved(base, unsaved, fresh).excluded_apps).toEqual([]);
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
