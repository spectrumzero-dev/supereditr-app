import { render, screen } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";
import type { Config } from "../lib/types";
import Privacy from "./Privacy.svelte";

// The app says which apps are installed and which are password managers; the test stands in for it.
vi.mock("./ipc", () => ({
  api: {
    appInfo: vi.fn(async (ids: string[]) =>
      Object.fromEntries(ids.filter((id) => id === "com.quillpoint.vault").map((id) => [id, { bundleId: id, name: "QuillPoint Vault", icon: null }])),
    ),
    passwordManagers: vi.fn(async (ids: string[]) => ids.filter((id) => id === "com.quillpoint.vault")),
  },
}));

describe("Privacy", () => {
  it("offers no Remove for a password manager, which is never read anyway", async () => {
    const config = { excluded_apps: ["com.quillpoint.vault", "com.quillpoint.notes"] } as Config;
    render(Privacy, { config, accessibility: true, onchange: () => {} });
    expect(await screen.findByText("QuillPoint Vault")).toBeTruthy();
    expect(await screen.findByText("Password manager")).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "Remove" })).toHaveLength(1);
  });
});
