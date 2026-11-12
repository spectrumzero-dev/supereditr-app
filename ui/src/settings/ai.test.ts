import { describe, expect, it } from "vitest";
import type { BuiltinAi, ProviderCheck } from "../lib/types";
import { aiState, needsAi } from "./ai";

const on: BuiltinAi = { supported: true, available: true, message: null };
const off: BuiltinAi = { supported: true, available: false, message: "Turn on Apple Intelligence" };
const unsupported: BuiltinAi = { supported: false, available: false, message: null };
const ready: ProviderCheck = { ready: true, problem: null, models: ["llama3.2"] };
const broken: ProviderCheck = { ready: false, problem: "Ollama isn’t running", models: [] };

describe("aiState", () => {
  it("has no AI when nothing is chosen and nothing is built in, as on Linux", () => {
    expect(aiState(false, null, unsupported)).toBe("none");
  });

  it("offers Apple Intelligence when it's supported but off", () => {
    expect(aiState(false, null, off)).toBe("off");
  });

  it("is ready with Apple Intelligence on, chosen AI or not", () => {
    expect(aiState(false, null, on)).toBe("ready");
    // Apple Intelligence answers when the chosen AI can't.
    expect(aiState(true, broken, on)).toBe("ready");
  });

  it("is ready with a chosen AI that works, with nothing built in", () => {
    expect(aiState(true, ready, unsupported)).toBe("ready");
    expect(aiState(true, ready, off)).toBe("ready");
  });

  it("has no AI when the chosen one fails its check and nothing built in stands in", () => {
    expect(aiState(true, broken, unsupported)).toBe("none");
    expect(aiState(true, broken, off)).toBe("off");
  });

  it("says nothing until the checks are back", () => {
    expect(aiState(false, null, null)).toBe("checking");
    expect(aiState(true, null, unsupported)).toBe("checking");
    expect(aiState(true, ready, null)).toBe("ready");
  });
});

describe("needsAi", () => {
  it("lets the rule-based checker do Proofread only", () => {
    expect(needsAi("proofread")).toBe(false);
    expect(needsAi("polish")).toBe(true);
    expect(needsAi("custom-abc123")).toBe(true);
  });
});
