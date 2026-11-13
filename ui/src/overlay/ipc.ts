import type { EngineEvent, SessionPayload, StatusPayload } from "../lib/types";
import type { RunRequest } from "./review";
import { demo } from "./demo";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T | undefined> {
  if (!inTauri) return demo.invoke(cmd, args) as T;
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

interface Handlers {
  session: (payload: SessionPayload) => void;
  /** `remembered`: an earlier review's result for the same selection, shown again. */
  engine: (id: number, presetId: string, event: EngineEvent, remembered: boolean) => void;
  status: (payload: StatusPayload) => void;
  focus: () => void;
  command: (command: string) => void;
  /** A key the keyboard tap took from the app under review, for the panel. */
  key: (key: PanelKeyPayload) => void;
  /** Review `id` couldn't be applied with the clipboard put aside: offer to replace anyway. */
  anyway: (id: number) => void;
}

export interface PanelKeyPayload {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

export const ipc = {
  /** Runs a style; see `runRequest` for what `steer` and `fresh` mean. */
  runPreset: (id: number, request: RunRequest) => invoke("run_preset", { id, ...request }),
  /** With `anyway`, the clipboard isn't put aside first and the fix stays on it (Windows). */
  apply: (id: number, text: string, anyway = false) => invoke<string>("apply_text", { id, text, anyway }),
  dismiss: (id: number | null) => invoke("dismiss", { id }),
  copy: (text: string) => invoke("copy_text", { text }),
  resize: (height: number) => invoke("resize_overlay", { height }),
  focus: () => invoke("focus_overlay"),
  /** The instruction box opened or closed: the panel takes keyboard focus only while it's open. */
  typing: (typing: boolean) => invoke("overlay_typing", { typing }),
  openSettings: () => invoke("open_settings", { section: null }),
  /** Adds a word to the word list; false when it's already there. */
  addWord: (word: string) => invoke<boolean>("add_word", { word }),
  listen(handlers: Handlers): () => void {
    if (!inTauri) return demo.listen(handlers);
    const stops: Array<() => void> = [];
    import("@tauri-apps/api/event").then(({ listen }) => {
      listen<SessionPayload>("overlay:session", (e) => handlers.session(e.payload)).then((u) => stops.push(u));
      listen<{ id: number; presetId: string; event: EngineEvent; remembered: boolean }>("overlay:engine", (e) =>
        handlers.engine(e.payload.id, e.payload.presetId, e.payload.event, e.payload.remembered),
      ).then((u) => stops.push(u));
      listen<StatusPayload>("overlay:status", (e) => handlers.status(e.payload)).then((u) => stops.push(u));
      listen("overlay:focus", () => handlers.focus()).then((u) => stops.push(u));
      listen<string>("overlay:command", (e) => handlers.command(e.payload)).then((u) => stops.push(u));
      listen<PanelKeyPayload>("overlay:key", (e) => handlers.key(e.payload)).then((u) => stops.push(u));
      listen<number>("overlay:replace-anyway", (e) => handlers.anyway(e.payload)).then((u) => stops.push(u));
    });
    return () => stops.forEach((stop) => stop());
  },
};
