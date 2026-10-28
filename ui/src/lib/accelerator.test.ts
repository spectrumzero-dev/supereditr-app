import { describe, expect, it } from "vitest";
import { acceleratorFromEvent, keycaps } from "./accelerator";

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
  });
});
