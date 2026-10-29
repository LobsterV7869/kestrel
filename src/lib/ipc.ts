import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { AppState, Bootstrap, Config } from "./types";

export const api = {
  bootstrap: () => invoke<Bootstrap>("bootstrap"),
  saveState: (state: AppState) => invoke<void>("save_state", { state }),
  logTiming: (name: string, elapsedMs: number, meta?: string) =>
    invoke<void>("log_timing", { name, elapsedMs, meta }),
  getConfig: () => invoke<Config>("get_config"),
  onBackgroundIdle: (cb: () => void): Promise<UnlistenFn> =>
    listen("kestrel://background-idle", cb),
};
