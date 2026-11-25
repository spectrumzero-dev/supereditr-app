import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, expect, it, vi } from "vitest";
import type { OllamaPullProgress, ProviderCheck, ProviderProfile } from "../lib/types";
import { api } from "./ipc";
import Models from "./Models.svelte";

const events = vi.hoisted(() => ({ pull: null as ((event: OllamaPullProgress) => void) | null }));
vi.mock("./ipc", async (importOriginal) => {
  const real = await importOriginal<typeof import("./ipc")>();
  return {
    ...real,
    api: {
      ...real.api,
      activateProvider: vi.fn(),
      adopt: vi.fn(),
      checkProvider: vi.fn(),
      pull: vi.fn(),
      removeProvider: vi.fn(),
      useBuiltinProvider: vi.fn(),
      on: vi.fn(async (_event, listener) => {
        events.pull = listener;
        return () => { events.pull = null; };
      }),
    },
  };
});

const ollama: ProviderProfile = {
  id: "ollama",
  kind: "ollama",
  label: "Ollama",
  base_url: "http://127.0.0.1:11434",
  model: "mistral",
};
const openAi: ProviderProfile = {
  id: "openai",
  kind: "open_ai",
  label: "OpenAI",
  base_url: "https://api.openai.com/v1",
  model: "gpt-6-luna",
};
const ready: ProviderCheck = { ready: true, problem: null, models: [] };

async function picker(active: ProviderProfile) {
  const { config } = await api.get();
  const saveNow = vi.fn(async () => true);
  const reload = vi.fn(async () => {});
  render(Models, {
    config: { ...structuredClone(config), providers: [ollama, openAi], active_provider: active.id },
    keys: {},
    saveNow,
    reload,
  });
  return { saveNow, reload };
}

const radio = (name: RegExp) => screen.getByRole<HTMLInputElement>("radio", { name });
const button = (name: string) => screen.getByRole<HTMLButtonElement>("button", { name });

function pendingCheck() {
  let resolve!: (check: ProviderCheck) => void;
  let reject!: (error: string) => void;
  const promise = new Promise<ProviderCheck>((pass, fail) => {
    resolve = pass;
    reject = fail;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.checkProvider).mockResolvedValue(ready);
});

it("keeps the working provider selected and prevents competing choices until a check fails", async () => {
  const activation = pendingCheck();
  vi.mocked(api.activateProvider).mockReturnValueOnce(activation.promise);
  await picker(openAi);
  await fireEvent.click(button("Scan"));
  const [adopt] = await screen.findAllByRole<HTMLButtonElement>("button", { name: "Use" });
  await fireEvent.click(radio(/Ollama/));

  const builtin = await screen.findByRole<HTMLInputElement>("radio", { name: /Apple Intelligence/ });
  expect(builtin.disabled).toBe(true);
  expect(adopt.disabled).toBe(true);
  expect(button("Remove").disabled).toBe(true);
  expect(radio(/^OpenAI API/).checked).toBe(true);
  await fireEvent.click(builtin);
  await fireEvent.click(adopt);
  expect(api.useBuiltinProvider).not.toHaveBeenCalled();
  expect(api.adopt).not.toHaveBeenCalled();
  expect(api.activateProvider).toHaveBeenCalledExactlyOnceWith(ollama.id);

  activation.reject("Server unavailable.");
  await waitFor(() => expect(builtin.disabled).toBe(false));
  expect(adopt.disabled).toBe(false);
  expect(radio(/^OpenAI API/).checked).toBe(true);
  expect(screen.getByText(/Server unavailable/)).toBeTruthy();
});

it("keeps Download busy after a layer finishes and checks the original request", async () => {
  const download = pendingCheck();
  vi.mocked(api.pull).mockReturnValueOnce(download.promise);
  const { saveNow, reload } = await picker(ollama);
  const model = screen.getByDisplayValue<HTMLInputElement>("gemma3:4b");
  await fireEvent.input(model, { target: { value: "qwen3:4b" } });
  await fireEvent.click(button("Download"));
  const [[requestId]] = vi.mocked(api.pull).mock.calls;
  expect(api.pull).toHaveBeenCalledExactlyOnceWith(requestId, ollama.id, ollama.base_url, "qwen3:4b");

  events.pull?.({ requestId: "other-download", progress: { status: "Other download", completed: 100, total: 100 } });
  expect(screen.queryByText("Other download")).toBeNull();
  events.pull?.({ requestId, progress: { status: "Layer complete", completed: 100, total: 100 } });
  await waitFor(() => expect(screen.getByText("Layer complete")).toBeTruthy());
  expect(button("Download").disabled).toBe(true);
  expect(button("Stop using").disabled).toBe(true);
  expect(model.disabled).toBe(true);
  await fireEvent.click(button("Download"));
  expect(api.pull).toHaveBeenCalledTimes(1);

  download.resolve(ready);
  await waitFor(() => expect(button("Download").disabled).toBe(false));
  expect(reload).toHaveBeenCalledTimes(1);
  expect(saveNow).not.toHaveBeenCalled();
  expect(api.activateProvider).not.toHaveBeenCalled();
});
