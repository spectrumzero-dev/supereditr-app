import { fireEvent, render, screen } from "@testing-library/svelte";
import { expect, it, vi } from "vitest";
import ThirdPartyLicences from "./ThirdPartyLicences.svelte";

vi.mock("./third-party-notices.json", () => ({
  default: {
    packages: [
      {
        kind: "crate", name: "demo", version: "1.0.0", licence: "MIT", fallback: true, source: "https://crates.io/crates/demo/1.0.0",
        notices: [
          { file: "NOTICE", text: "own" },
          { file: "MIT.txt", text: "mit", origin: "spdx" },
        ],
      },
    ],
    texts: { own: "Own notice", mit: "MIT text" },
  },
}));

function shownFiles() {
  return [...document.querySelectorAll("h2")].map((heading) => {
    const label = heading.nextElementSibling;
    return [heading.textContent, label?.tagName === "P" ? label.textContent : null];
  });
}

it("says a package ships without a licence file and labels its standard text", async () => {
  render(ThirdPartyLicences, { back: () => {} });
  await fireEvent.click(screen.getByRole("button", { name: "demo 1.0.0 MIT" }));
  expect(screen.getByText("This package ships without a licence file.")).toBeTruthy();
  expect(shownFiles()).toEqual([
    ["NOTICE", null],
    ["MIT.txt", "Standard SPDX text."],
  ]);
});
