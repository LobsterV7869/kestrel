import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  AccountSummary,
  AppState,
  AuthPoll,
  Bootstrap,
  Config,
  CreateInstanceResult,
  DeviceCodeInfo,
  InstallItem,
  InstallOutcome,
  InstallPlan,
  InstallProgress,
  InstalledMod,
  Loader,
  LoaderInstallResult,
  LoaderStatus,
  LoaderVersion,
  McVersion,
  ModsCatalog,
  ModUpdate,
  ModVersion,
  ProjectDetail,
  SearchResult,
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

  /* Instances and loaders */
  listMcVersions: () => invoke<McVersion[]>("list_mc_versions"),
  listLoaderVersions: (loader: Loader, mcVersion: string) =>
    invoke<LoaderVersion[]>("list_loader_versions", { loader, mcVersion }),
  createInstance: (
    name: string,
    mcVersion: string,
    loader: Loader,
    loaderVersion: string | null,
  ) =>
    invoke<CreateInstanceResult>("create_instance", {
      name,
      mcVersion,
      loader,
      loaderVersion,
    }),
  installLoader: (instanceId: string) =>
    invoke<LoaderInstallResult>("install_loader", { instanceId }),
  loaderStatus: (instanceId: string) =>
    invoke<LoaderStatus>("loader_status", { instanceId }),

  /* Mods */
  modsCatalog: (instanceId: string) =>
    invoke<ModsCatalog>("mods_catalog", { instanceId }),
  modsSearch: (
    instanceId: string,
    query: string,
    category: string,
    offset: number,
    limit: number,
  ) =>
    invoke<SearchResult>("mods_search", {
      instanceId,
      query,
      category,
      offset,
      limit,
    }),
  modsProject: (projectId: string) =>
    invoke<ProjectDetail>("mods_project", { projectId }),
  modsVersions: (instanceId: string, projectId: string) =>
    invoke<ModVersion[]>("mods_versions", { instanceId, projectId }),
  modsPlan: (instanceId: string, items: InstallItem[]) =>
    invoke<InstallPlan>("mods_plan", { instanceId, items }),
  modsInstall: (instanceId: string, items: InstallItem[]) =>
    invoke<InstallOutcome>("mods_install", { instanceId, items }),
  modsCheckUpdates: (instanceId: string) =>
    invoke<ModUpdate[]>("mods_check_updates", { instanceId }),
  modsUpdate: (instanceId: string, projectIds: string[]) =>
    invoke<InstallOutcome>("mods_update", { instanceId, projectIds }),
  modsInstalled: (instanceId: string) =>
    invoke<InstalledMod[]>("mods_installed", { instanceId }),
  modsSetEnabled: (instanceId: string, fileName: string, enabled: boolean) =>
    invoke<InstalledMod[]>("mods_set_enabled", {
      instanceId,
      fileName,
      enabled,
    }),
  modsRemove: (instanceId: string, fileName: string) =>
    invoke<InstalledMod[]>("mods_remove", { instanceId, fileName }),
  modsImportLocal: (instanceId: string, path: string) =>
    invoke<InstalledMod[]>("mods_import_local", { instanceId, path }),
  modsImportBytes: (instanceId: string, fileName: string, bytes: number[]) =>
    invoke<InstalledMod[]>("mods_import_bytes", {
      instanceId,
      fileName,
      bytes,
    }),
  onModsProgress: (cb: (progress: InstallProgress) => void): Promise<UnlistenFn> =>
    listen<InstallProgress>("kestrel://mods-progress", (event) =>
      cb(event.payload),
    ),
  onModsChanged: (cb: (instanceId: string) => void): Promise<UnlistenFn> =>
    listen<string>("kestrel://mods-changed", (event) => cb(event.payload)),
};
