import { fireEvent, render, screen } from "@testing-library/svelte";
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

it.each([
  ["Get Started", "No AI set up"],
  ["Styles", "Styles other than Proofread need an AI."],
])("opens Models from the no-AI notice on %s", async (page, notice) => {
  vi.spyOn(api, "builtinAi").mockResolvedValue({ supported: false, available: false, message: null });
  render(App);
  await fireEvent.click(await screen.findByRole("button", { name: page }));
  expect(await screen.findByText(notice, { exact: false })).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Set up AI" }));
  expect(await screen.findByRole("heading", { name: "Models" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Scan" })).toBeTruthy();
});

it.each([
  ["macos", "button", /⌃⌥E or F6/],
  ["windows", "button", /Ctrl\+Shift\+F2/],
  ["linux", "combobox", /free on Omarchy/],
])("names each shortcut control and describes it on %s", async (platform, role, reviewHint) => {
  vi.spyOn(api, "get").mockResolvedValue({ ...await api.get(), platform });
  render(App);
  await fireEvent.click(await screen.findByRole("button", { name: "Shortcuts" }));
  expect(screen.getByRole(role, { name: /^Review selection\b/, description: reviewHint })).toBeTruthy();
  expect(screen.getByRole(role, { name: /^Quick fix\b/, description: /Applies your default style/ })).toBeTruthy();
  expect(screen.getByRole("checkbox", { name: "With nothing selected, use the sentence before the cursor", description: /Automatic suggestions never do this/ })).toBeTruthy();
  expect(screen.getByRole("checkbox", { name: "Always show key tips in the suggestion panel", description: /first few reviews/ })).toBeTruthy();
});

it("associates General, Dictionary and Styles controls with their captions", async () => {
  render(App);
  await fireEvent.click(await screen.findByRole("button", { name: "General" }));
  expect(screen.getByRole("checkbox", { name: "Suggest as soon as I select text", description: /shortcut only/ })).toBeTruthy();
  expect(screen.getByRole("spinbutton", { name: "Minimum selection", description: /characters Shorter selections/ })).toBeTruthy();
  expect(screen.getByRole("spinbutton", { name: "Maximum selection", description: /characters Longer selections/ })).toBeTruthy();
  expect(screen.getByRole("checkbox", { name: "Keep formatting when applying fixes", description: /Undo takes one step per fix/ })).toBeTruthy();
  expect(screen.getByRole("checkbox", { name: "Show response times" })).toBeTruthy();
  expect(screen.getByRole("checkbox", { name: "Open at login", description: /menu bar/ })).toBeTruthy();
  expect(screen.getByRole("checkbox", { name: "Keep a diagnostic log", description: /Turning it off deletes the log/ })).toBeTruthy();

  await fireEvent.click(screen.getByRole("button", { name: "Dictionary" }));
  expect(screen.getByRole("textbox", { name: "Words to keep" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "English spelling", description: /rule-based checker and your AI/ })).toBeTruthy();

  await fireEvent.click(screen.getByRole("button", { name: "Styles" }));
  expect(screen.getByRole("textbox", { name: "Standing instructions", description: /Applied to every style/ })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Default style", description: /Used for automatic suggestions and quick fix/ })).toBeTruthy();
  expect(screen.getByRole("textbox", { name: "Name" })).toBeTruthy();
  expect(screen.getByRole("textbox", { name: "Instruction" })).toBeTruthy();
  expect(screen.getByRole("button", { name: /^Shortcut\b/, description: /Runs this style on the selection/ })).toBeTruthy();
});

it("names the model setup fields for the provider and check result on screen", async () => {
  const payload = await api.get();
  const [claude] = payload.config.providers;
  const ollama = { id: "ollama", kind: "ollama", label: "Ollama", base_url: "http://localhost:11434", model: "gemma3:4b" } as const;
  payload.config = {
    ...payload.config,
    providers: [claude, { ...claude, id: "other-claude" }, ollama],
    active_provider: claude.id,
    onboarded: true,
  };
  vi.spyOn(api, "get").mockResolvedValue(payload);
  vi.spyOn(api, "checkProvider").mockImplementation(async (id) => ({
    ready: true, problem: null, code: null, models: id === ollama.id ? [ollama.model] : [],
  }));
  vi.spyOn(api, "activateProvider").mockResolvedValue({ ready: true, problem: null, code: null, models: [ollama.model] });
  render(App);

  expect(await screen.findByRole("combobox", { name: "Account" })).toBeTruthy();
  expect(screen.getByLabelText("API key")).toBeInstanceOf(HTMLInputElement);
  expect(screen.getByRole("textbox", { name: "Model" })).toBeTruthy();
  await fireEvent.click(screen.getByRole("radio", { name: "Ollama", description: /runs on/ }));
  expect(await screen.findByRole("combobox", { name: "Model" })).toBeTruthy();
  expect(screen.getByRole("textbox", { name: "Server URL" })).toBeTruthy();
  expect(screen.getByRole("textbox", { name: "Download a model", description: /Small models/ })).toBeTruthy();
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
