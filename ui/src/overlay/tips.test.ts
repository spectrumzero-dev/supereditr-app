import { describe, expect, it } from "vitest";
import { closeTips, countTip, TIP_REVIEWS } from "./tips";

function memory() {
  const items = new Map<string, string>();
  return { getItem: (k: string) => items.get(k) ?? null, setItem: (k: string, v: string) => void items.set(k, v) };
}

describe("key tips", () => {
  it("show for the first few reviews only", () => {
    const store = memory();
    for (let i = 0; i < TIP_REVIEWS; i++) expect(countTip(false, store)).toBe(true);
    expect(countTip(false, store)).toBe(false);
  });

  it("stay closed once the author closes them", () => {
    const store = memory();
    expect(countTip(false, store)).toBe(true);
    closeTips(store);
    expect(countTip(false, store)).toBe(false);
  });

  it("always show when the setting asks, without using up the first few", () => {
    const store = memory();
    for (let i = 0; i < TIP_REVIEWS + 2; i++) expect(countTip(true, store)).toBe(true);
    expect(countTip(false, store)).toBe(true);
    closeTips(store);
    expect(countTip(true, store)).toBe(true);
  });

  it("stay hidden when storage is unavailable", () => {
    const broken = { getItem: () => { throw new Error("blocked"); }, setItem: () => {} };
    expect(countTip(false, broken)).toBe(false);
    expect(() => closeTips({ getItem: () => null, setItem: () => { throw new Error("blocked"); } })).not.toThrow();
  });
});
