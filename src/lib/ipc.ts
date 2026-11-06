import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  AccountSummary,
  AppState,
  AuthPoll,
  Bootstrap,
  Config,
  DeviceCodeInfo,
} from "./types";

export const api = {
  bootstrap: () => invoke<Bootstrap>("bootstrap"),
  saveState: (state: AppState) => invoke<void>("save_state", { state }),
  logTiming: (name: string, elapsedMs: number, meta?: string) =>
    invoke<void>("log_timing", { name, elapsedMs, meta }),
  getConfig: () => invoke<Config>("get_config"),
  onBackgroundIdle: (cb: () => void): Promise<UnlistenFn> =>
    listen("kestrel://background-idle", cb),
  authStart: () => invoke<DeviceCodeInfo>("auth_start"),
  authPoll: () => invoke<AuthPoll>("auth_poll"),
  authRestore: () => invoke<AccountSummary | null>("auth_restore"),
  authLogout: () => invoke<void>("auth_logout"),
  openExternal: (url: string) => invoke<void>("open_external", { url }),
  onAccount: (cb: (account: AccountSummary | null) => void): Promise<UnlistenFn> =>
    listen<AccountSummary | null>("kestrel://account", (event) => cb(event.payload)),
  onAuthError: (cb: (message: string) => void): Promise<UnlistenFn> =>
    listen<string>("kestrel://auth-error", (event) => cb(event.payload)),
};
