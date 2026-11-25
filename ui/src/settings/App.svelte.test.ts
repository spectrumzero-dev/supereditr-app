import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, expect, it, vi } from "vitest";
import App from "./App.svelte";
import { api } from "./ipc";

// Settings first mounts Get Started, which measures its banner.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

it("opens the bundled third-party licences from About and shows the Apache licence for Harper", async () => {
  render(App);
  await fireEvent.click(await screen.findByRole("button", { name: "About" }));
  await fireEvent.click(screen.getByRole("button", { name: "Third-party licences" }));
  expect(await screen.findByRole("heading", { name: "Third-party licences" })).toBeTruthy();

  const harper = screen.getByRole("button", { name: /^harper-core [\d.]+ Apache-2\.0$/ });
  expect(harper.getAttribute("aria-expanded")).toBe("false");
  expect(document.querySelector("pre")).toBeNull();
  await fireEvent.click(harper);
  expect(document.querySelector("pre")?.textContent).toContain("TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION");
  expect(screen.getByText("This package ships without a licence file.")).toBeTruthy();
  expect(screen.getByText("Standard SPDX text.")).toBeTruthy();

  await fireEvent.click(screen.getByRole("button", { name: "Back to About" }));
  expect(screen.getByRole("heading", { name: "About SuperEditr" })).toBeTruthy();
});

afterEach(() => {
  vi.restoreAllMocks();
});

it("keeps a failed edit and saves it with Retry after merging a change made outside Settings", async () => {
  const original = await api.get();
  const fresh = {
    ...original,
    config: { ...original.config, onboarded: true, words: ["Rhyan"], excluded_apps: ["com.example.QuillPoint"] },
    revision: 1,
  };
  const get = vi.spyOn(api, "get").mockResolvedValue(fresh);
  const save = vi.spyOn(api, "save")
    .mockRejectedValueOnce("The settings file changed on disk.")
    .mockResolvedValueOnce({ revision: 3, problems: [], recovery: null });

  render(App);
  await screen.findByRole("heading", { name: "Models" });
  await fireEvent.click(await screen.findByRole("button", { name: "Dictionary" }));
  await fireEvent.input(screen.getByPlaceholderText(/Rhyan/), { target: { value: "Brynlee" } });
  await fireEvent.click(screen.getByRole("button", { name: "Add" }));
  expect((await screen.findByRole("alert")).textContent).toContain("Couldn’t save your changes.");
  expect(screen.getByRole("button", { name: "Remove Brynlee" })).toBeTruthy();

  get.mockResolvedValue({
    ...fresh,
    revision: 2,
    config: { ...fresh.config, words: ["Rhyan", "Kyndal"], excluded_apps: [...fresh.config.excluded_apps, "com.example.Rhyan"] },
  });
  await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await screen.findByText("Saved");
  expect(get).toHaveBeenLastCalledWith(true);
  expect(save).toHaveBeenLastCalledWith(expect.objectContaining({
    words: ["Rhyan", "Kyndal", "Brynlee"],
    excluded_apps: ["com.example.QuillPoint", "com.example.Rhyan"],
  }), 2);
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
});

it("offers to reload an unreadable settings file and restores controls when it is repaired", async () => {
  const original = await api.get();
  const recovery = {
    path: "/example/settings.json",
    issues: ["Settings must contain a valid JSON object."],
    writable: false,
    exclusionsSafe: false,
    destinationSafe: false,
  };
  const get = vi.spyOn(api, "get").mockResolvedValue({ ...original, recovery });
  const save = vi.spyOn(api, "save");
  render(App);
  expect((await screen.findByRole("status")).textContent).toContain("SuperEditr couldn’t load your settings");
  await fireEvent.click(screen.getByRole("button", { name: "Dictionary" }));
  expect(screen.queryByRole("heading", { name: "Dictionary" })).toBeNull();

  get.mockResolvedValue({ ...original, recovery: null });
  await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByRole("heading", { name: "Dictionary" })).toBeTruthy();
  expect(save).not.toHaveBeenCalled();
});

it("keeps a model edit pending until the affected profile can be recovered", async () => {
  const original = await api.get();
  const profile = original.config.providers[0];
  const fresh = {
    ...original,
    config: { ...original.config, onboarded: true, active_provider: profile.id },
    revision: 1,
  };
  const get = vi.spyOn(api, "get").mockResolvedValue(fresh);
  const save = vi.spyOn(api, "save")
    .mockRejectedValueOnce("The settings file changed on disk.")
    .mockResolvedValueOnce({ revision: 4, problems: [], recovery: null });
  render(App);

  const model = await screen.findByRole("textbox");
  await fireEvent.input(model, { target: { value: "new-model" } });
  await fireEvent.change(model);
  expect((await screen.findByRole("alert")).textContent).toContain("Couldn’t save your changes.");

  get.mockImplementation(async () => ({
    ...fresh,
    revision: 2,
    config: { ...fresh.config, providers: [] },
    recovery: {
      path: "/example/settings.json", issues: ["providers", "active_provider"],
      writable: true, exclusionsSafe: true, destinationSafe: false,
    },
  }));
  await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByText(/Your changes are still pending/)).toBeTruthy();
  expect(save).toHaveBeenCalledTimes(1);

  get.mockResolvedValue({
    ...fresh, revision: 3,
    config: { ...fresh.config, providers: [{ ...profile, label: "QuillPoint" }] },
  });
  await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await screen.findByText("Saved");
  expect(save).toHaveBeenLastCalledWith(expect.objectContaining({
    providers: [{ ...profile, label: "QuillPoint", model: "new-model" }],
  }), 3);
});

it("shows Retry when adding or removing a profile fails", async () => {
  const original = await api.get();
  const profile = original.config.providers[0];
  const fresh = { ...original, config: { ...original.config, onboarded: true, active_provider: profile.id } };
  const get = vi.spyOn(api, "get").mockResolvedValue(fresh);
  const add = vi.spyOn(api, "addProvider").mockRejectedValue("The settings file changed on disk.");
  const remove = vi.spyOn(api, "removeProvider").mockRejectedValue("The settings file changed on disk.");
  render(App);

  await fireEvent.click(await screen.findByRole("radio", { name: /Ollama/ }));
  expect((await screen.findByRole("alert")).textContent).toContain("Couldn’t add the AI.");
  expect(add).toHaveBeenCalledWith("ollama");
  await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  expect(get).toHaveBeenLastCalledWith(true);

  await fireEvent.click(screen.getByRole("button", { name: "Remove" }));
  expect((await screen.findByRole("alert")).textContent).toContain("Couldn’t remove the AI.");
  expect(remove).toHaveBeenCalledWith(profile.id);
  expect(screen.getByRole("button", { name: "Remove" })).toBeTruthy();
});

async function openLicences() {
  render(App);
  await fireEvent.click(await screen.findByRole("button", { name: "About" }));
  await fireEvent.click(screen.getByRole("button", { name: "Third-party licences" }));
  await screen.findByRole("heading", { name: "Third-party licences" });
}

it("opens each third-party package's source from its expanded row", async () => {
  const open = vi.spyOn(window, "open").mockImplementation(() => null);
  await openLicences();
  await fireEvent.click(screen.getByRole("button", { name: /^cssparser [\d.]+ MPL-2\.0$/ }));
  const source = screen.getByRole("button", { name: /^https:\/\/crates\.io\/crates\/cssparser\/[\d.]+$/ });
  await fireEvent.click(source);
  expect(open).toHaveBeenCalledWith(source.textContent, "_blank");

  await fireEvent.click(screen.getByRole("button", { name: /^clsx [\d.]+ MIT$/ }));
  expect(screen.getByRole("button", { name: /^https:\/\/www\.npmjs\.com\/package\/clsx\/v\/[\d.]+$/ })).toBeTruthy();
});

it("lists the Rust standard library", async () => {
  await openLicences();
  await fireEvent.click(screen.getByRole("button", { name: /^Rust standard library [\d.]+ MIT OR Apache-2\.0$/ }));
  expect(screen.getByRole("button", { name: /^https:\/\/github\.com\/rust-lang\/rust\/tree\/[\d.]+\/library$/ })).toBeTruthy();
});

it("lists the WebView2 loader with the shipped crate version", async () => {
  await openLicences();
  await fireEvent.click(screen.getByRole("button", {
    name: /^WebView2 loader \(Windows\) \(shipped with webview2-com-sys [\d.]+\) BSD-3-Clause$/,
  }));
  expect(screen.getByRole("button", { name: "https://www.nuget.org/packages/Microsoft.Web.WebView2" })).toBeTruthy();
});
