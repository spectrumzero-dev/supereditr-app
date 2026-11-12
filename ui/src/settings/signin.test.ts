import { afterEach, describe, expect, it } from "vitest";
import { problemText, waitForSignIn } from "./signin";
import { setPlatform } from "../lib/platform";

/** A clock that moves only when the wait sleeps. */
function clock() {
  let t = 0;
  return { now: () => t, sleep: async (ms: number) => void (t += ms) };
}

describe("Claude sign-in", () => {
  afterEach(() => setPlatform("macos"));

  it("words the sign-in problem per platform, and other problems as given", () => {
    const check = { ready: false, problem: "Sign in to Claude Code: run `claude` in Terminal and use /login", code: "claude_sign_in", models: [] };
    setPlatform("windows");
    expect(problemText(check)).toBe("Claude Code isn’t signed in to your Claude account yet");
    setPlatform("macos");
    expect(problemText(check)).toBe(check.problem);
    expect(problemText({ ready: false, problem: "Add an API key", code: null, models: [] })).toBe("Add an API key");
  });

  it("stops as soon as the status flips to signed in", async () => {
    const c = clock();
    const answers = [false, false, true];
    let asked = 0;
    const end = await waitForSignIn(async () => answers[asked++], { every: 2000, limit: 300_000, visible: () => true, cancelled: () => false, ...c });
    expect([end, asked, c.now()]).toEqual(["signed-in", 3, 6000]);
  });

  it("gives up after five minutes, and on cancel or a hidden window", async () => {
    const c = clock();
    let asked = 0;
    const timedOut = await waitForSignIn(async () => (asked++, false), { every: 2000, limit: 300_000, visible: () => true, cancelled: () => false, ...c });
    expect([timedOut, asked]).toEqual(["timed-out", 150]);
    let cancel = false;
    const cancelled = await waitForSignIn(async () => ((cancel = true), false), { every: 2000, limit: 300_000, visible: () => true, cancelled: () => cancel, ...clock() });
    expect(cancelled).toBe("cancelled");
    expect(await waitForSignIn(async () => true, { every: 2000, limit: 300_000, visible: () => false, cancelled: () => false, ...clock() })).toBe("hidden");
  });
});
