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

export interface DeviceCodeInfo {
  userCode: string;
  verificationUri: string;
  expiresIn: number;
  interval: number;
  message: string;
}

export type AuthStatus =
  | "pending"
  | "slowDown"
  | "expired"
  | "declined"
  | "success";

export interface AuthPoll {
  status: AuthStatus;
  account: AccountSummary | null;
}

/* ------------------------------------------------------------------ */
/* Instances, loaders and mods                                         */
/* ------------------------------------------------------------------ */

export interface McVersion {
  id: string;
  kind: string;
  releaseTime: string;
}

export interface LoaderVersion {
  version: string;
  stable: boolean;
}

export interface LoaderStatus {
  ready: boolean;
  installedVersion: string | null;
  message: string | null;
}

export interface InstalledLoader {
  loader: Loader;
  mcVersion: string;
  loaderVersion: string;
  profileId: string;
  mainClass: string;
  libraries: number;
}

export interface CreateInstanceResult {
  state: AppState;
  warning: string | null;
}

export interface LoaderInstallResult {
  state: AppState;
  loader: InstalledLoader;
}

export interface InstanceSummary {
  id: string;
  name: string;
  mcVersion: string;
  loader: Loader;
  loaderVersion: string | null;
  gameDir: string;
}

export interface ProjectSummary {
  id: string;
  slug: string;
  title: string;
  description: string;
  author: string;
  iconUrl: string | null;
  categories: string[];
  loaders: string[];
  gameVersions: string[];
  downloads: number;
  license: string;
  clientSide: string;
  latestVersion: string | null;
  featured: boolean;
}

export interface GalleryImage {
  url: string;
  title: string | null;
  description: string | null;
}

/** Rust flattens `ProjectSummary` into `ProjectDetail`. */
export interface ProjectDetail extends ProjectSummary {
  body: string;
  gallery: GalleryImage[];
  sourceUrl: string | null;
  issuesUrl: string | null;
  licenseName: string;
  licenseUrl: string | null;
}

export interface ModDependency {
  projectId: string | null;
  versionId: string | null;
  dependencyType: string;
  fileName: string | null;
}

export interface ModFile {
  filename: string;
  url: string;
  sha1: string;
  sha512: string;
  size: number;
  primary: boolean;
}

export interface ModVersion {
  id: string;
  projectId: string;
  name: string;
  versionNumber: string;
  versionType: string;
  gameVersions: string[];
  loaders: string[];
  dependencies: ModDependency[];
  files: ModFile[];
  datePublished: string;
}

export interface SearchResult {
  hits: ProjectSummary[];
  offset: number;
  limit: number;
  totalHits: number;
}

export interface InstallItem {
  projectId: string;
  versionId?: string | null;
  featureId?: string | null;
}

export interface PlannedMod {
  projectId: string;
  slug: string;
  title: string;
  author: string;
  iconUrl: string | null;
  versionId: string;
  versionNumber: string;
  filename: string;
  url: string;
  sha1: string;
  sha512: string;
  size: number;
  license: string;
  featureId: string | null;
  isDependency: boolean;
}

export interface InstallPlan {
  instanceId: string;
  mcVersion: string;
  loader: Loader;
  items: PlannedMod[];
  warnings: string[];
}

export interface InstallProgress {
  transactionId: string;
  phase: string;
  completedFiles: number;
  totalFiles: number;
  downloadedBytes: number;
  totalBytes: number;
  current: string | null;
  message: string | null;
}

export interface InstalledMod {
  projectId: string;
  slug: string;
  title: string;
  versionId: string;
  versionNumber: string;
  fileName: string;
  sha1: string | null;
  sha512: string | null;
  size: number;
  installedAt: number;
  featureId: string | null;
  source: string;
  enabled: boolean;
  present: boolean;
  iconUrl: string | null;
  author: string;
  license: string;
}

export interface InstallOutcome {
  transactionId: string;
  instanceId: string;
  installed: InstalledMod[];
  warnings: string[];
}

export interface ModUpdate {
  projectId: string;
  slug: string;
  title: string;
  currentVersion: string;
  currentVersionId: string;
  latestVersion: string;
  latestVersionId: string;
  fileName: string;
  iconUrl: string | null;
}

export interface EssentialFeature {
  id: string;
  name: string;
  description: string;
  keybind: string;
  category: string;
  essential: boolean;
  project: ProjectSummary | null;
  version: ModVersion | null;
  available: boolean;
  reason: string | null;
  installed: InstalledMod | null;
  update: ModUpdate | null;
}

export interface ModsCatalog {
  instance: InstanceSummary | null;
  loaderReady: boolean;
  loaderError: string | null;
  features: EssentialFeature[];
  installed: InstalledMod[];
  updates: ModUpdate[];
  warnings: string[];
}
