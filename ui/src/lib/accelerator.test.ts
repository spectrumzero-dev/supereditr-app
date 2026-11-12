import { describe, expect, it } from "vitest";
import { acceleratorFromEvent, keycaps, windowsKeyName } from "./accelerator";
import { keySymbols } from "./diff";
import { setKeyLabels, setPlatform } from "./platform";
import { afterEach } from "vitest";

const ev = (code: string, mods: Partial<KeyboardEvent> = {}) => ({ code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods }) as KeyboardEvent;

describe("acceleratorFromEvent", () => {
  it("builds Tauri accelerators", () => {
    expect(acceleratorFromEvent(ev("KeyE", { ctrlKey: true, altKey: true }))).toBe("Control+Alt+E");
    expect(acceleratorFromEvent(ev("Digit2", { metaKey: true, shiftKey: true }))).toBe("Shift+Command+2");
    expect(acceleratorFromEvent(ev("F5"))).toBe("F5");
  });
  it("rejects shortcuts that would capture normal typing", () => {
    expect(acceleratorFromEvent(ev("KeyE"))).toBeNull();
    expect(acceleratorFromEvent(ev("KeyE", { shiftKey: true }))).toBeNull();
    expect(acceleratorFromEvent(ev("ShiftLeft", { shiftKey: true }))).toBeNull();
  });
});

describe("keycaps", () => {
  it("puts each modifier and key on its own cap", () => {
    expect(keycaps("Control+Alt+E")).toEqual(["⌃", "⌥", "E"]);
    expect(keycaps("Command+E Command+E")).toEqual(["⌘", "E", "E"]);
    expect(keycaps("")).toEqual([]);
    expect(keycaps("Super+Alt+E", false)).toEqual(["Super", "Alt", "E"]);
  });
});

describe("on Windows", () => {
  afterEach(() => {
    setPlatform("macos");
    setKeyLabels({});
  });
  const win = (keyCode: number, code: string, mods: Partial<KeyboardEvent> = {}) =>
    ({ ...ev(code, mods), keyCode }) as KeyboardEvent;

  it("records the virtual key Windows reports, not the US position", () => {
    setPlatform("windows");
    // German: the key in the US [ position types ü, which Windows reports as VK_OEM_1.
    expect(acceleratorFromEvent(win(0xba, "BracketLeft", { ctrlKey: true, shiftKey: true }))).toBe("Control+Shift+VkBA");
    expect(acceleratorFromEvent(win(0x71, "F2", { ctrlKey: true, shiftKey: true }))).toBe("Control+Shift+F2");
    // AZERTY: the key in the US Q position types A.
    expect(acceleratorFromEvent(win(0x41, "KeyQ", { ctrlKey: true, altKey: true }))).toBe("Control+Alt+A");
    expect(acceleratorFromEvent(win(0x20, "Space", { ctrlKey: true, altKey: true }))).toBe("Control+Alt+Space");
    expect(acceleratorFromEvent(win(0x11, "ControlLeft", { ctrlKey: true }))).toBeNull();
  });

  it("names keys and labels them by the layout", () => {
    expect(windowsKeyName(0xde)).toBe("VkDE");
    expect(windowsKeyName(0x37)).toBe("7");
    expect(windowsKeyName(0x87)).toBe("F24");
    setPlatform("windows");
    setKeyLabels({ VkBA: "Ü" });
    expect(keySymbols("Control+Shift+VkBA")).toBe("Ctrl+Shift+Ü");
    expect(keySymbols("Control+Shift+VkDE")).toBe("Ctrl+Shift+Key DE");
  });
});
