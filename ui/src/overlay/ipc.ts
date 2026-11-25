import type { Command, EventName, EventPayloads } from "../lib/ipc-names";
import type { EngineEvent, PanelKeyPayload, SessionPayload, StatusPayload } from "../lib/types";
import type { RunRequest } from "./review";
import { demo } from "./demo";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function invoke<T>(cmd: Command, args?: Record<string, unknown>): Promise<T | undefined> {
  if (!inTauri) return demo.invoke(cmd, args) as T;
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

interface Handlers {
  session: (payload: SessionPayload) => void;
  /** `run`: which run of the session the event is from. `remembered`: an earlier review's result
   *  for the same selection, shown again. */
  engine: (id: number, presetId: string, run: number, event: EngineEvent, remembered: boolean) => void;
  status: (payload: StatusPayload) => void;
  focus: () => void;
  command: (command: string) => void;
  /** A key the keyboard tap took from the app under review, for the panel. */
  key: (key: PanelKeyPayload) => void;
  /** Review `id` couldn't be applied with the clipboard put aside: offer to replace anyway. */
  anyway: (id: number) => void;
}

export const ipc = {
  /** Runs a style as run number `run` of review `id`; see `runRequest` for what `steer` and `fresh` mean. */
  runPreset: (id: number, run: number, request: RunRequest) => invoke("run_preset", { id, run, ...request }),
  /** With `anyway`, the clipboard isn't put aside first and the fix stays on it (Windows). */
  apply: (id: number, text: string, anyway = false) => invoke<string>("apply_text", { id, text, anyway }),
  checkProtectedTokens: (id: number, text: string) => invoke<string[]>("check_protected_tokens", { id, text }),
  dismiss: (id: number | null) => invoke("dismiss", { id }),
  copy: (text: string) => invoke("copy_text", { text }),
  resize: (height: number) => invoke("resize_overlay", { height }),
  focus: () => invoke("focus_overlay"),
  /** The instruction box opened or closed: the panel takes keyboard focus only while it's open. */
  typing: (typing: boolean) => invoke("overlay_typing", { typing }),
  openSettings: () => invoke("open_settings", { section: null }),
  /** Adds a word to the word list; false when it's already there. */
  addWord: (word: string) => invoke<boolean>("add_word", { word }),
  /** The key tips were closed: turns "always show" off in settings. */
  hideTips: () => invoke("hide_tips"),
  listen(handlers: Handlers): () => void {
    if (!inTauri) return demo.listen(handlers);
    const stops: Array<() => void> = [];
    import("@tauri-apps/api/event").then(({ listen: tauriListen }) => {
      const listen = <E extends EventName>(event: E, handler: (payload: EventPayloads[E]) => void) =>
        tauriListen<EventPayloads[E]>(event, (e) => handler(e.payload)).then((u) => stops.push(u));
      listen("overlay:session", (payload) => handlers.session(payload));
      listen("overlay:engine", (p) => handlers.engine(p.id, p.presetId, p.run, p.event, p.remembered));
      listen("overlay:status", (payload) => handlers.status(payload));
      listen("overlay:focus", () => handlers.focus());
      listen("overlay:command", (command) => handlers.command(command));
      listen("overlay:key", (key) => handlers.key(key));
      listen("overlay:replace-anyway", (id) => handlers.anyway(id));
    });
    return () => stops.forEach((stop) => stop());
  },
};
