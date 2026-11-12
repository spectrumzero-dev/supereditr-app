import { afterEach, describe, expect, it } from "vitest";
import { isLinux, isMac, isWindows, setPlatform, words } from "./platform";
import { keySymbols } from "./diff";

describe("platform", () => {
  afterEach(() => setPlatform("macos"));

  it("follows the platform the app reports", () => {
    setPlatform("windows");
    expect([isMac(), isLinux(), isWindows()]).toEqual([false, false, true]);
    setPlatform("linux");
    expect([isMac(), isLinux(), isWindows()]).toEqual([false, true, false]);
    setPlatform("freebsd");
    expect(isLinux()).toBe(true);
    setPlatform("macos");
    expect(isMac()).toBe(true);
  });

  it("uses each platform's own words", () => {
    setPlatform("windows");
    expect(words().device).toBe("this PC");
    expect(words().keyStore).toBe("Windows Credential Manager");
    expect(words().tray).toBe("the notification area");
    expect(words().copyKey).toBe("Ctrl+C");
    setPlatform("macos");
    expect(words().copyKey).toBe("⌘C");
  });

  it("shows Alt+Enter as the only apply key on Windows, and Windows' key names", () => {
    setPlatform("windows");
    expect(words().applyKeys).toEqual(["Alt+Enter"]);
    expect([words().tabKey, words().escKey]).toEqual(["Tab", "Esc"]);
    setPlatform("macos");
    expect(words().applyKeys).toEqual(["↩", "⌥↩"]);
    expect([words().tabKey, words().escKey]).toEqual(["tab", "esc"]);
    setPlatform("linux");
    expect(words().applyKeys).toEqual(["Enter", "Alt+Enter"]);
    expect([words().tabKey, words().escKey]).toEqual(["tab", "esc"]);
  });

  it("names the logo key for the platform", () => {
    setPlatform("windows");
    expect(keySymbols("Control+Shift+F2")).toBe("Ctrl+Shift+F2");
    expect(keySymbols("Super+Alt+E")).toBe("Win+Alt+E");
    setPlatform("linux");
    expect(keySymbols("Super+Alt+E")).toBe("Super+Alt+E");
  });
});
