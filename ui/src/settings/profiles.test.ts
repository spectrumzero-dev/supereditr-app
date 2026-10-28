import { describe, expect, it } from "vitest";
import type { ProviderProfile } from "../lib/types";
import { profileFor, setupAction } from "./profiles";

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
