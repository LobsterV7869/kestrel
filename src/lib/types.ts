export type Loader = "vanilla" | "fabric" | "quilt" | "neoforge" | "forge";
export type Preset = "optimized" | "vanilla" | "custom";
export type AccountKind = "microsoft";

export interface Profile {
  id: string;
  name: string;
  mcVersion: string;
  loader: Loader;
  loaderVersion: string | null;
  javaPath: string | null;
  memoryMb: number | null;
  jvmArgs: string[];
  gameArgs: string[];
  icon: string | null;
  preset: Preset;
  createdAt: number;
  lastPlayed: number | null;
}

export interface AccountSummary {
  id: string;
  username: string;
  uuid: string;
  kind: AccountKind;
}

export interface AppState {
  schema: number;
  profiles: Profile[];
  accounts: AccountSummary[];
  selectedProfile: string | null;
  lastServer: string | null;
  updatedAt: number;
}

export interface LaunchRequest {
  profile: string | null;
  join: string | null;
}

export interface Bootstrap {
  state: AppState;
  processStartEpochMs: number;
  nowEpochMs: number;
  backendReadyMs: number;
  appName: string;
  appVersion: string;
  legal: string;
  azureConfigured: boolean;
  launch: LaunchRequest;
}

export interface Config {
  azureClientId: string;
  defaultMemoryMb: number | null;
  globalJvmArgs: string[];
  autoJoinLastServer: boolean;
  enableAppcds: boolean;
  prewarmPageCache: boolean;
  closeToTray: boolean;
}
