import { fireEvent, render, screen } from "@testing-library/svelte";
import { expect, it, vi } from "vitest";
import ThirdPartyLicences from "./ThirdPartyLicences.svelte";

// No shipped package is allow-listed or has both its own and a repository's notice, so these rows
// are made up here.
vi.mock("./third-party-notices.json", () => ({
  default: {
    packages: [
      {
        kind: "crate", name: "allowed", version: "1.0.0", licence: "MIT", fallback: true, source: "https://crates.io/crates/allowed/1.0.0",
        notices: [
          { file: "NOTICE", text: "own" },
          { file: "MIT.txt", text: "mit", origin: "spdx" },
        ],
      },
      {
        kind: "crate", name: "mixed", version: "2.0.0", licence: "MIT", fallback: false, source: "https://crates.io/crates/mixed/2.0.0",
        upstream: { repository: "https://example.com/mixed", commit: "0123456789abcdef0123456789abcdef01234567" },
        notices: [
          { file: "NOTICE", text: "own" },
          { file: "LICENSE", text: "mit", origin: "repository" },
          { file: "NOTICE", text: "theirs", origin: "repository" },
        ],
      },
    ],
    texts: { own: "Own notice", mit: "MIT text", theirs: "Repository notice" },
  },
}));

function shownFiles() {
  return [...document.querySelectorAll("h2")].map((heading) => {
    const label = heading.nextElementSibling;
    return [heading.textContent, label?.tagName === "P" ? label.textContent : null];
  });
}

it("says an allow-listed package ships without a licence file and labels its standard text", async () => {
  render(ThirdPartyLicences, { back: () => {} });
  await fireEvent.click(screen.getByRole("button", { name: "allowed 1.0.0 MIT" }));
  expect(screen.getByText("This package ships without a licence file.")).toBeTruthy();
  expect(shownFiles()).toEqual([
    ["NOTICE", null],
    ["MIT.txt", "Standard SPDX text."],
  ]);
});

it("shows a package's own notice beside the repository's notice of the same name", async () => {
  render(ThirdPartyLicences, { back: () => {} });
  await fireEvent.click(screen.getByRole("button", { name: "mixed 2.0.0 MIT" }));
  const repository = "From its repository, https://example.com/mixed, at commit 0123456789ab.";
  expect(shownFiles()).toEqual([
    ["NOTICE", null],
    ["LICENSE", repository],
    ["NOTICE", repository],
  ]);
  expect([...document.querySelectorAll("pre")].map((pre) => pre.textContent)).toEqual(["Own notice", "MIT text", "Repository notice"]);
});
