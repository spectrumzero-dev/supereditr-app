import { describe, expect, it } from "vitest";
import type { AppInfo } from "../lib/types";
import { excludedRows, withApps } from "./excluded";

const app = (bundleId: string, name: string): AppInfo => ({ bundleId, name, icon: null });

describe("excludedRows", () => {
  it("lists installed apps by name, then the rest by bundle ID", () => {
    const ids = ["com.quillpoint.vault", "com.apple.Terminal", "org.example.shell", "com.apple.Passwords"];
    const known = {
      "com.quillpoint.vault": null,
      "com.apple.Terminal": app("com.apple.Terminal", "Terminal"),
      "org.example.shell": null,
      "com.apple.Passwords": app("com.apple.Passwords", "Passwords"),
    };
    expect(excludedRows(ids, known).map((row) => row.app?.name ?? row.id)).toEqual([
      "Passwords",
      "Terminal",
      "com.quillpoint.vault",
      "org.example.shell",
    ]);
  });

  it("leaves out an app until its lookup returns", () => {
    const rows = excludedRows(["com.apple.Terminal", "com.apple.Notes"], { "com.apple.Notes": null });
    expect(rows).toEqual([{ id: "com.apple.Notes", app: null }]);
  });
});

describe("withApps", () => {
  it("adds new bundle IDs, trimmed", () => {
    expect(withApps(["com.apple.Terminal"], [" com.apple.Notes "])).toEqual(["com.apple.Terminal", "com.apple.Notes"]);
  });

  it("skips blanks and IDs already listed in any case", () => {
    const list = ["com.apple.Terminal"];
    expect(withApps(list, ["", "COM.APPLE.TERMINAL", "com.apple.Notes", "com.apple.notes"])).toEqual([
      "com.apple.Terminal",
      "com.apple.Notes",
    ]);
  });
});
