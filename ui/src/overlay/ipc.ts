import type { EngineEvent, SessionPayload, StatusPayload } from "../lib/types";
import { demo } from "./demo";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T | undefined> {
  if (!inTauri) return demo.invoke(cmd, args) as T;
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

interface Handlers {
  session: (payload: SessionPayload) => void;
  engine: (id: number, presetId: string, event: EngineEvent) => void;
  status: (payload: StatusPayload) => void;
  focus: () => void;
  command: (command: string) => void;
}

export const ipc = {
  runPreset: (id: number, presetId: string, steer: string | null) => invoke("run_preset", { id, presetId, steer }),
  apply: (id: number, text: string) => invoke<string>("apply_text", { id, text }),
  dismiss: (id: number | null) => invoke("dismiss", { id }),
  copy: (text: string) => invoke("copy_text", { text }),
  resize: (height: number) => invoke("resize_overlay", { height }),
  focus: () => invoke("focus_overlay"),
  openSettings: () => invoke("open_settings", { section: null }),
  /** Adds a word to the word list; false when it's already there. */
  addWord: (word: string) => invoke<boolean>("add_word", { word }),
  listen(handlers: Handlers): () => void {
    if (!inTauri) return demo.listen(handlers);
    const stops: Array<() => void> = [];
    import("@tauri-apps/api/event").then(({ listen }) => {
      listen<SessionPayload>("overlay:session", (e) => handlers.session(e.payload)).then((u) => stops.push(u));
      listen<{ id: number; presetId: string; event: EngineEvent }>("overlay:engine", (e) =>
        handlers.engine(e.payload.id, e.payload.presetId, e.payload.event),
      ).then((u) => stops.push(u));
      listen<StatusPayload>("overlay:status", (e) => handlers.status(e.payload)).then((u) => stops.push(u));
      listen("overlay:focus", () => handlers.focus()).then((u) => stops.push(u));
      listen<string>("overlay:command", (e) => handlers.command(e.payload)).then((u) => stops.push(u));
    });
    return () => stops.forEach((stop) => stop());
  },
};
