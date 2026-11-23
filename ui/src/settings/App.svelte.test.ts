import { fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, expect, it, vi } from "vitest";
import App from "./App.svelte";

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
  // Harper's crate archives ship no licence file; the text comes from its repository.
  expect(screen.getByText("This package ships without a licence file.")).toBeTruthy();
  expect(screen.getByText(/^From its repository, https:\/\/github\.com\/automattic\/harper, at commit [0-9a-f]{12}\.$/)).toBeTruthy();
  expect(screen.queryByText(/Standard SPDX text/)).toBeNull();

  await fireEvent.click(screen.getByRole("button", { name: "Back to About" }));
  expect(screen.getByRole("heading", { name: "About SuperEditr" })).toBeTruthy();
});

afterEach(() => {
  vi.restoreAllMocks();
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

it("labels each file of a mixed row by its own origin", async () => {
  await openLicences();
  await fireEvent.click(screen.getByRole("button", { name: /^objc2-app-kit [\d.]+ Zlib OR Apache-2\.0 OR MIT$/ }));
  // Each file's heading is followed by the label of its origin.
  const files = [...document.querySelectorAll("h2")].map((heading) => [heading.textContent, heading.nextElementSibling?.textContent]);
  const repository = expect.stringMatching(/^From its repository, https:\/\/github\.com\/madsmtm\/objc2, at commit [0-9a-f]{12}\.$/);
  expect(files).toEqual([
    ["LICENSE.md", repository],
    ["Apache-2.0.txt", "Standard SPDX text."],
    ["MIT.txt", "Standard SPDX text."],
    ["Zlib.txt", "Standard SPDX text."],
  ]);
  expect(screen.getAllByText("This package ships without a licence file.")).toHaveLength(1);
});

it("lists the Rust standard library", async () => {
  await openLicences();
  await fireEvent.click(screen.getByRole("button", { name: /^Rust standard library [\d.]+ MIT OR Apache-2\.0$/ }));
  expect(screen.getByRole("button", { name: /^https:\/\/github\.com\/rust-lang\/rust\/tree\/[\d.]+\/library$/ })).toBeTruthy();
});
