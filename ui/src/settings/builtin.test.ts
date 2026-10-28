import { afterEach, describe, expect, it, vi } from "vitest";
import type { BuiltinAi } from "../lib/types";
import { watchBuiltinAi } from "./builtin";

const off: BuiltinAi = { supported: true, available: false, message: "Turn on Apple Intelligence" };
const on: BuiltinAi = { supported: true, available: true, message: null };
const flush = () => new Promise((r) => setTimeout(r, 0));

afterEach(() => vi.useRealTimers());

describe("watchBuiltinAi", () => {
  it("notices Apple Intelligence turned on while the page stays open", async () => {
    let current = off;
    const seen: BuiltinAi[] = [];
    const window = new EventTarget();
    const stop = watchBuiltinAi(async () => current, (s) => seen.push(s), window, 1_000_000);
    await flush();
    expect(seen.at(-1)).toEqual(off);
    current = on;
    window.dispatchEvent(new Event("focus"));
    await flush();
    expect(seen.at(-1)).toEqual(on);
    stop();
  });

  it("polls only while unavailable, and stops when the page closes", async () => {
    vi.useFakeTimers();
    let current = off;
    const fetch = vi.fn(async () => current);
    const stop = watchBuiltinAi(fetch, () => {}, new EventTarget(), 100);
    await vi.advanceTimersByTimeAsync(0);
    current = on;
    await vi.advanceTimersByTimeAsync(100);
    expect(fetch).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(500);
    expect(fetch).toHaveBeenCalledTimes(2);
    stop();
  });
});
