import { fireEvent, render, screen } from "@testing-library/svelte";
import { expect, it } from "vitest";
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

  await fireEvent.click(screen.getByRole("button", { name: "Back to About" }));
  expect(screen.getByRole("heading", { name: "About SuperEditr" })).toBeTruthy();
});
