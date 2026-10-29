import { describe, expect, it } from "vitest";
import type { ProviderProfile } from "../lib/types";
import { earlyAccess, profileFor, setupAction } from "./profiles";

const server = (id: string, base_url: string): ProviderProfile =>
  ({ id, kind: "open_ai_compatible", label: "Other server", base_url, model: "m" }) as ProviderProfile;
const providers = [server("compatible", "http://a/v1"), server("compatible-2", "http://b/v1")];

describe("profileFor", () => {
  it("shows the AI in use, not the first of its kind", () => {
    expect(profileFor(providers, "open_ai_compatible", undefined, "compatible-2")?.base_url).toBe("http://b/v1");
  });
  it("shows the one picked in the row's account menu", () => {
    expect(profileFor(providers, "open_ai_compatible", "compatible", "compatible-2")?.id).toBe("compatible");
  });
  it("falls back to the first, and to none for an unused kind", () => {
    expect(profileFor(providers, "open_ai_compatible", undefined, null)?.id).toBe("compatible");
    expect(profileFor(providers, "gemini", undefined, null)).toBeNull();
  });
});

describe("setupAction", () => {
  it("offers Use for a second account of the kind in use", () => {
    const shown = profileFor(providers, "open_ai_compatible", "compatible", "compatible-2")!;
    expect(setupAction(shown, "compatible-2")).toBe("use");
    expect(setupAction(providers[1], "compatible-2")).toBe("check");
  });
});

describe("earlyAccess", () => {
  it("labels cloud AIs and a server that isn't on this Mac", () => {
    expect(earlyAccess("anthropic", null)).toBe(true);
    expect(earlyAccess("claude_cli", null)).toBe(true);
    expect(earlyAccess("open_ai_compatible", server("s", "https://openrouter.ai/api/v1"))).toBe(true);
  });
  it("leaves local AIs and an unfinished address unlabeled", () => {
    expect(earlyAccess("ollama", null)).toBe(false);
    expect(earlyAccess("open_ai_compatible", null)).toBe(false);
    expect(earlyAccess("open_ai_compatible", server("s", "http://localhost:1234/v1"))).toBe(false);
    expect(earlyAccess("open_ai_compatible", server("s", "http://127.0.0.1:1234/v1"))).toBe(false);
    expect(earlyAccess("open_ai_compatible", server("s", "http://[::1]:1234/v1"))).toBe(false);
    expect(earlyAccess("open_ai_compatible", server("s", "openrouter"))).toBe(false);
  });
});
