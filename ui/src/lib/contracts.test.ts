// The shared fixtures in tests/contracts, which Rust tests read too: what this UI writes or
// computes must match what the Rust shell reads or computes.
import { afterEach, describe, expect, it } from "vitest";
import acceleratorsJson from "../../../tests/contracts/accelerators.json?raw";
import diffApplyJson from "../../../tests/contracts/diff-apply.json?raw";
import ipcNamesJson from "../../../tests/contracts/ipc-names.json?raw";
import { acceleratorFromEvent, windowsKeyName } from "./accelerator";
import { applyView } from "./diff";
import { commands, events } from "./ipc-names";
import { setPlatform, type Platform } from "./platform";
import type { DiffView } from "./types";

interface KeyEvent {
  code: string;
  key: string;
  keyCode?: string;
  ctrlKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
  metaKey?: boolean;
}

interface Accelerators {
  cases: { name: string; platforms: Platform[]; event: KeyEvent; accelerator: string | null }[];
  doubled: { name: string; event: KeyEvent; accelerator: string }[];
  windowsKeyNames: { vk: string; name: string }[];
}

interface DiffApply {
  cases: { name: string; view: DiffView; applied: { rejected: number[]; text: string }[] }[];
}

const accelerators: Accelerators = JSON.parse(acceleratorsJson);
const diffApply: DiffApply = JSON.parse(diffApplyJson);
const ipcNames: { commands: string[]; events: string[] } = JSON.parse(ipcNamesJson);

const keydown = (e: KeyEvent) =>
  ({
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    metaKey: false,
    ...e,
    keyCode: e.keyCode === undefined ? 0 : Number(e.keyCode),
  }) as unknown as KeyboardEvent;

describe("accelerators.json", () => {
  afterEach(() => setPlatform("macos"));

  it("lists cases", () => {
    expect(accelerators.cases.length).toBeGreaterThan(0);
    expect(accelerators.doubled.length).toBeGreaterThan(0);
  });

  for (const c of accelerators.cases) {
    for (const platform of c.platforms) {
      it(`records ${c.name} on ${platform}`, () => {
        setPlatform(platform);
        expect(acceleratorFromEvent(keydown(c.event))).toBe(c.accelerator);
      });
    }
  }

  // ShortcutInput.svelte writes a double press as the recorded chord, a space, and the chord again.
  for (const c of accelerators.doubled) {
    it(`records the chord of ${c.name}`, () => {
      setPlatform("macos");
      const chord = acceleratorFromEvent(keydown(c.event));
      expect(`${chord} ${chord}`).toBe(c.accelerator);
    });
  }

  // keys::name also names Esc, Backspace, Delete, PrintScreen and Pause, and gives every other key
  // VkXX; windowsKeyName returns null for those, so they aren't in the shared list.
  it("names Windows virtual keys as keys::name does", () => {
    expect(accelerators.windowsKeyNames.length).toBeGreaterThan(0);
    for (const { vk, name } of accelerators.windowsKeyNames) expect(windowsKeyName(Number(vk)), vk).toBe(name);
  });
});

describe("diff-apply.json", () => {
  it("lists cases", () => expect(diffApply.cases.length).toBeGreaterThan(0));

  for (const c of diffApply.cases) {
    it(`applies ${c.name} as Rust's apply does`, () => {
      for (const { rejected, text } of c.applied) expect(applyView(c.view, new Set(rejected)), `${rejected}`).toBe(text);
    });
  }
});

// src-tauri/src/ipc_contract.rs checks the same lists against the commands the app registers and the
// events it emits.
describe("ipc-names.json", () => {
  it("names every command the app registers, as ipc-names.ts does", () => {
    expect([...commands].sort()).toEqual([...ipcNames.commands].sort());
  });

  it("names every event the app emits, as ipc-names.ts does", () => {
    expect([...events].sort()).toEqual([...ipcNames.events].sort());
  });
});
