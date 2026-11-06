import {
  createEffect,
  createMemo,
  createSignal,
  For,
  on,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { api } from "../lib/ipc";
import type {
  AppState,
  EssentialFeature,
  InstallItem,
  InstallPlan,
  InstallProgress,
  InstalledMod,
  ModsCatalog,
  ModUpdate,
  Profile,
  ProjectSummary,
} from "../lib/types";
import {
  formatBytes,
  formatCount,
  loaderLabel,
  phaseLabel,
  titleCase,
} from "../lib/format";
import ModDetailModal from "./ModDetailModal";

type Tab = "essentials" | "browse" | "installed" | "updates";

const BROWSE_CATEGORIES = [
  "all",
  "optimization",
  "utility",
  "adventure",
  "decoration",
  "equipment",
  "magic",
  "technology",
  "worldgen",
  "storage",
  "transportation",
  "mobs",
  "library",
];

interface Props {
  profile: Profile | null;
  onStateChange: (state: AppState) => void;
  onCreateInstance: () => void;
  onNotice: (message: string) => void;
}

export default function ModsPage(props: Props) {
  const [catalog, setCatalog] = createSignal<ModsCatalog | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [loadError, setLoadError] = createSignal<string | null>(null);
  const [tab, setTab] = createSignal<Tab>("essentials");
  const [featureCategory, setFeatureCategory] = createSignal("all");
  const [busy, setBusy] = createSignal<string | null>(null);
  const [planning, setPlanning] = createSignal(false);
  const [installing, setInstalling] = createSignal<string | null>(null);
  const [progress, setProgress] = createSignal<InstallProgress | null>(null);
  const [pending, setPending] = createSignal<{
    plan: InstallPlan;
    items: InstallItem[];
    label: string;
  } | null>(null);
  const [actionWarnings, setActionWarnings] = createSignal<string[]>([]);
  const [detailId, setDetailId] = createSignal<string | null>(null);
  const [importing, setImporting] = createSignal(false);

  const [searchQuery, setSearchQuery] = createSignal("");
  const [searchCategory, setSearchCategory] = createSignal("all");
  const [results, setResults] = createSignal<ProjectSummary[]>([]);
  const [searchTotal, setSearchTotal] = createSignal(0);
  const [searching, setSearching] = createSignal(false);
  const [searchError, setSearchError] = createSignal<string | null>(null);

  let fileInput: HTMLInputElement | undefined;
  let catalogGeneration = 0;
  let searchGeneration = 0;
  let searchTimer: number | undefined;
  let unlistenProgress: UnlistenFn | undefined;
  let unlistenChanged: UnlistenFn | undefined;
  let unlistenDrop: UnlistenFn | undefined;

  onMount(async () => {
    unlistenProgress = await api
      .onModsProgress((event) => setProgress(event))
      .catch(() => undefined);
    unlistenChanged = await api
      .onModsChanged((id) => {
        if (id === props.profile?.id) void loadCatalog(id);
      })
      .catch(() => undefined);
    try {
      unlistenDrop = await getCurrentWebview().onDragDropEvent((event) => {
        if (event.payload.type !== "drop") return;
        const jars = event.payload.paths.filter((path) =>
          path.toLowerCase().endsWith(".jar"),
        );
        if (!jars.length) {
          if (event.payload.paths.length) {
            props.onNotice("Drop a .jar mod file to import it.");
          }
          return;
        }
        void importPaths(jars);
      });
    } catch {
      // Drag and drop is unavailable outside a Tauri webview.
    }
  });

  onCleanup(() => {
    if (searchTimer) clearTimeout(searchTimer);
    unlistenProgress?.();
    unlistenChanged?.();
    unlistenDrop?.();
  });

  const features = () => catalog()?.features ?? [];
  const installed = () => catalog()?.installed ?? [];
  const updates = () => catalog()?.updates ?? [];
  const loaderReady = () => catalog()?.loaderReady ?? false;

  createEffect(
    on(
      () => props.profile?.id,
      (id) => {
        catalogGeneration++;
        searchGeneration++;
        setCatalog(null);
        setLoadError(null);
        setResults([]);
        setSearchTotal(0);
        setSearchError(null);
        setDetailId(null);
        setPending(null);
        setActionWarnings([]);
        setTab("essentials");
        setFeatureCategory("all");
        if (id) void loadCatalog(id);
      },
    ),
  );

  createEffect(
    on(
      [
        searchQuery,
        searchCategory,
        tab,
        loaderReady,
        () => props.profile?.id,
      ],
      () => {
        if (searchTimer) clearTimeout(searchTimer);
        if (tab() !== "browse" || !loaderReady() || !props.profile) return;
        searchTimer = window.setTimeout(() => void runSearch(true), 350);
      },
    ),
  );

  const categories = createMemo(() => {
    const seen = new Set<string>();
    for (const feature of features()) seen.add(feature.category);
    return ["all", ...Array.from(seen)];
  });

  const visibleFeatures = createMemo(() => {
    const category = featureCategory();
    return features().filter(
      (feature) => category === "all" || feature.category === category,
    );
  });

  const installedFeatureCount = createMemo(
    () => features().filter((feature) => feature.installed).length,
  );

  const essentialRemaining = createMemo(
    () =>
      features().filter(
        (feature) => feature.essential && feature.available && !feature.installed,
      ).length,
  );

  const detailFeature = createMemo(() =>
    features().find(
      (feature) =>
        feature.project?.id === detailId() ||
        feature.installed?.projectId === detailId(),
    ),
  );

  const detailInstalled = createMemo(
    () =>
      installed().find((mod) => mod.projectId === detailId()) ??
      detailFeature()?.installed ??
      null,
  );

  const detailUpdate = createMemo(
    () =>
      updates().find((update) => update.projectId === detailId()) ??
      detailFeature()?.update ??
      null,
  );

  const percent = createMemo(() => {
    const current = progress();
    if (!current) return 0;
    if (current.totalBytes > 0) {
      return Math.min(
        100,
        Math.round((current.downloadedBytes / current.totalBytes) * 100),
      );
    }
    if (current.totalFiles > 0) {
      return Math.min(
        100,
        Math.round((current.completedFiles / current.totalFiles) * 100),
      );
    }
    return 0;
  });

  const progressText = createMemo(() => {
    const current = progress();
    if (!current) return "Starting…";
    if (current.totalBytes > 0) {
      return `${formatBytes(current.downloadedBytes)} of ${formatBytes(
        current.totalBytes,
      )}`;
    }
    return `${current.completedFiles} of ${current.totalFiles} files`;
  });

  async function loadCatalog(instanceId: string) {
    const generation = ++catalogGeneration;
    setLoading(true);
    setLoadError(null);
    try {
      const next = await api.modsCatalog(instanceId);
      if (generation !== catalogGeneration || props.profile?.id !== instanceId) {
        return;
      }
      setCatalog(next);
    } catch (err) {
      if (generation === catalogGeneration) setLoadError(String(err));
    } finally {
      if (generation === catalogGeneration) setLoading(false);
    }
  }

  function mergeInstalled(entries: InstalledMod[]) {
    const current = catalog();
    if (!current) return;
    const byFile = new Map(current.installed.map((mod) => [mod.fileName, mod]));
    for (const mod of entries) byFile.set(mod.fileName, mod);
    setCatalog({ ...current, installed: Array.from(byFile.values()) });
  }

  async function planInstall(items: InstallItem[], label: string) {
    const profile = props.profile;
    if (!profile) return;
    if (!loaderReady()) {
      props.onNotice(
        catalog()?.loaderError ?? "Install the mod loader before adding mods.",
      );
      return;
    }
    if (!items.length) {
      props.onNotice("Nothing to install.");
      return;
    }
    setPlanning(true);
    setActionWarnings([]);
    try {
      const plan = await api.modsPlan(profile.id, items);
      if (!plan.items.length) {
        props.onNotice("Nothing to install; everything selected is already present.");
        return;
      }
      setPending({ plan, items, label });
    } catch (err) {
      props.onNotice(String(err));
    } finally {
      setPlanning(false);
    }
  }

  async function confirmInstall() {
    const profile = props.profile;
    const request = pending();
    if (!profile || !request) return;
    setPending(null);
    setInstalling(request.label);
    setProgress(null);
    setActionWarnings([]);
    try {
      const outcome = await api.modsInstall(profile.id, request.items);
      mergeInstalled(outcome.installed);
      setActionWarnings(outcome.warnings);
      const dependencies = request.plan.items.filter(
        (item) => item.isDependency,
      ).length;
      const count = outcome.installed.length;
      props.onNotice(
        `Installed ${count} mod${count === 1 ? "" : "s"}${
          dependencies
            ? ` including ${dependencies} dependenc${
                dependencies === 1 ? "y" : "ies"
              }`
            : ""
        }.`,
      );
      void loadCatalog(profile.id);
    } catch (err) {
      props.onNotice(String(err));
    } finally {
      setInstalling(null);
      setProgress(null);
    }
  }

  function installFeature(feature: EssentialFeature) {
    if (!feature.project) {
      props.onNotice(feature.reason ?? "This feature is unavailable.");
      return;
    }
    void planInstall(
      [
        {
          projectId: feature.project.id,
          versionId: feature.version?.id ?? null,
          featureId: feature.id,
        },
      ],
      `Installing ${feature.name}`,
    );
  }

  function updateFeature(feature: EssentialFeature) {
    if (!feature.update) return;
    void updateMods([feature.update.projectId]);
  }

  function installEssentials() {
    const items = features()
      .filter(
        (feature) =>
          feature.essential &&
          feature.available &&
          !feature.installed &&
          feature.project,
      )
      .map((feature) => ({
        projectId: feature.project!.id,
        versionId: feature.version?.id ?? null,
        featureId: feature.id,
      }));
    if (!items.length) {
      props.onNotice("All essential features are already installed.");
      return;
    }
    void planInstall(items, "Installing Kestrel Essentials");
  }

  async function updateMods(projectIds: string[]) {
    const profile = props.profile;
    if (!profile || !projectIds.length) return;
    setInstalling(
      projectIds.length === 1
        ? "Updating mod"
        : `Updating ${projectIds.length} mods`,
    );
    setProgress(null);
    setActionWarnings([]);
    try {
      const outcome = await api.modsUpdate(profile.id, projectIds);
      mergeInstalled(outcome.installed);
      const current = catalog();
      if (current) {
        setCatalog({
          ...current,
          updates: current.updates.filter(
            (update) => !projectIds.includes(update.projectId),
          ),
        });
      }
      setActionWarnings(outcome.warnings);
      const updated = outcome.installed.filter((mod) =>
        projectIds.includes(mod.projectId),
      ).length;
      props.onNotice(
        updated
          ? `Updated ${updated} mod${updated === 1 ? "" : "s"}.`
          : "Everything is already up to date.",
      );
      void loadCatalog(profile.id);
    } catch (err) {
      props.onNotice(String(err));
    } finally {
      setInstalling(null);
      setProgress(null);
    }
  }

  async function installLoader() {
    const profile = props.profile;
    if (!profile) return;
    setBusy("loader");
    try {
      const result = await api.installLoader(profile.id);
      props.onStateChange(result.state);
      props.onNotice(
        `${loaderLabel(result.loader.loader)} ${
          result.loader.loaderVersion
        } installed for Minecraft ${result.loader.mcVersion}.`,
      );
      await loadCatalog(profile.id);
    } catch (err) {
      props.onNotice(String(err));
    } finally {
      setBusy(null);
    }
  }

  async function toggleMod(mod: InstalledMod) {
    const profile = props.profile;
    if (!profile) return;
    setBusy(mod.fileName);
    try {
      const next = await api.modsSetEnabled(profile.id, mod.fileName, !mod.enabled);
      mergeInstalled(next);
    } catch (err) {
      props.onNotice(String(err));
    } finally {
      setBusy(null);
    }
  }

  async function removeMod(mod: InstalledMod) {
    const profile = props.profile;
    if (!profile) return;
    if (
      !window.confirm(
        `Remove ${mod.title}? The jar is moved to the instance trash folder.`,
      )
    ) {
      return;
    }
    setBusy(mod.fileName);
    try {
      const next = await api.modsRemove(profile.id, mod.fileName);
      mergeInstalled(next);
      props.onNotice(`Removed ${mod.title}.`);
      void loadCatalog(profile.id);
    } catch (err) {
      props.onNotice(String(err));
    } finally {
      setBusy(null);
    }
  }

  async function checkUpdates() {
    const profile = props.profile;
    if (!profile) return;
    setBusy("updates");
    try {
      const next = await api.modsCheckUpdates(profile.id);
      const current = catalog();
      if (current) setCatalog({ ...current, updates: next });
      props.onNotice(
        next.length
          ? `${next.length} update${next.length === 1 ? "" : "s"} available.`
          : "All mods are up to date.",
      );
    } catch (err) {
      props.onNotice(String(err));
    } finally {
      setBusy(null);
    }
  }

  async function importPaths(paths: string[]) {
    const profile = props.profile;
    if (!profile) return;
    setImporting(true);
    try {
      let next: InstalledMod[] = [];
      for (const path of paths) {
        next = await api.modsImportLocal(profile.id, path);
      }
      mergeInstalled(next);
      props.onNotice(
        `Imported ${paths.length} mod${paths.length === 1 ? "" : "s"}.`,
      );
      void loadCatalog(profile.id);
    } catch (err) {
      props.onNotice(String(err));
    } finally {
      setImporting(false);
    }
  }

  async function onFilePicked(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    if (!file.name.toLowerCase().endsWith(".jar")) {
      props.onNotice("Only .jar mods can be imported.");
      return;
    }
    if (file.size > 128 * 1024 * 1024) {
      props.onNotice("That jar is larger than 128 MB.");
      return;
    }
    const profile = props.profile;
    if (!profile) return;
    setImporting(true);
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const next = await api.modsImportBytes(
        profile.id,
        file.name,
        Array.from(bytes),
      );
      mergeInstalled(next);
      props.onNotice(`Imported ${file.name}.`);
      void loadCatalog(profile.id);
    } catch (err) {
      props.onNotice(String(err));
    } finally {
      setImporting(false);
    }
  }

  async function runSearch(reset: boolean) {
    const profile = props.profile;
    if (!profile || !loaderReady()) return;
    const generation = ++searchGeneration;
    const offset = reset ? 0 : results().length;
    setSearching(true);
    setSearchError(null);
    try {
      const response = await api.modsSearch(
        profile.id,
        searchQuery(),
        searchCategory(),
        offset,
        24,
      );
      if (generation !== searchGeneration) return;
      setResults(
        reset ? response.hits : [...results(), ...response.hits],
      );
      setSearchTotal(response.totalHits);
    } catch (err) {
      if (generation === searchGeneration) setSearchError(String(err));
    } finally {
      if (generation === searchGeneration) setSearching(false);
    }
  }

  return (
    <section class="mods">
      <Show
        when={props.profile}
        fallback={
          <div class="empty-state tall">
            <span class="brand-mark large">K</span>
            <h2>No instance selected</h2>
            <p class="muted">
              Create a Fabric or Quilt instance to install Kestrel Client
              Features and browse mods.
            </p>
            <button class="btn primary" onClick={props.onCreateInstance}>
              Create instance
            </button>
          </div>
        }
      >
        {(profile) => (
          <>
            <header class="mods-head">
              <div class="titles">
                <h1>Mods</h1>
                <p class="muted">
                  {profile().name} · {profile().mcVersion} ·{" "}
                  {loaderLabel(profile().loader)}
                  <Show when={profile().loaderVersion}>
                    {" "}
                    · loader {profile().loaderVersion}
                  </Show>
                </p>
              </div>
              <div class="actions">
                <button class="btn ghost" onClick={props.onCreateInstance}>
                  New instance
                </button>
                <button
                  class="btn ghost"
                  disabled={importing()}
                  onClick={() => fileInput?.click()}
                  title="Import a local .jar mod"
                >
                  {importing() ? "Importing…" : "Import .jar"}
                </button>
                <button
                  class="btn ghost"
                  disabled={loading()}
                  onClick={() => void loadCatalog(profile().id)}
                >
                  {loading() ? "Refreshing…" : "Refresh"}
                </button>
              </div>
            </header>

            <input
              ref={(element) => (fileInput = element)}
              type="file"
              accept=".jar,application/java-archive"
              class="hidden-file"
              onChange={(event) => void onFilePicked(event)}
            />

            <Show when={loadError()}>
              <div class="banner danger">
                <div class="banner-text">
                  <strong>Couldn't load mods</strong>
                  <p class="hint">{loadError()}</p>
                </div>
                <button
                  class="btn ghost"
                  onClick={() => void loadCatalog(profile().id)}
                >
                  Try again
                </button>
              </div>
            </Show>

            <Show when={catalog() && !loaderReady()}>
              <div class="banner warning">
                <div class="banner-icon">!</div>
                <div class="banner-text">
                  <strong>
                    {profile().loader === "vanilla"
                      ? "This instance is vanilla"
                      : "Mod loader not ready"}
                  </strong>
                  <p class="hint">
                    {catalog()?.loaderError ??
                      "Install the loader before adding mods."}
                  </p>
                </div>
                <Show
                  when={
                    profile().loader === "fabric" ||
                    profile().loader === "quilt"
                  }
                  fallback={
                    <button class="btn primary" onClick={props.onCreateInstance}>
                      Create modded instance
                    </button>
                  }
                >
                  <button
                    class="btn primary"
                    disabled={busy() === "loader"}
                    onClick={() => void installLoader()}
                  >
                    {busy() === "loader"
                      ? "Installing…"
                      : `Install ${loaderLabel(profile().loader)}`}
                  </button>
                </Show>
              </div>
            </Show>

            <Show when={actionWarnings().length}>
              <div class="banner subtle">
                <div class="banner-text">
                  <strong>Install notes</strong>
                  <For each={actionWarnings()}>
                    {(warning) => <p class="hint">{warning}</p>}
                  </For>
                </div>
                <button class="close" onClick={() => setActionWarnings([])}>
                  ×
                </button>
              </div>
            </Show>

            <nav class="mods-tabs" aria-label="Mods sections">
              <button
                classList={{ active: tab() === "essentials" }}
                onClick={() => setTab("essentials")}
              >
                Essentials
                <span class="tab-count">
                  {installedFeatureCount()}/{features().length}
                </span>
              </button>
              <button
                classList={{ active: tab() === "browse" }}
                onClick={() => setTab("browse")}
              >
                Browse
              </button>
              <button
                classList={{ active: tab() === "installed" }}
                onClick={() => setTab("installed")}
              >
                Installed
                <span class="tab-count">{installed().length}</span>
              </button>
              <button
                classList={{ active: tab() === "updates" }}
                onClick={() => setTab("updates")}
              >
                Updates
                <Show when={updates().length}>
                  <span class="tab-count accent">{updates().length}</span>
                </Show>
              </button>
            </nav>

            <Show when={loading() && !catalog()}>
              <div class="mods-loading">
                <span class="spinner" />
                <p class="muted">Resolving Kestrel Client Features…</p>
              </div>
            </Show>

            <Show when={catalog()}>
              <Show when={tab() === "essentials"}>
                <div class="essentials">
                  <div class="essentials-hero">
                    <div class="essentials-mark">K</div>
                    <div class="essentials-copy">
                      <h2>Kestrel Client Features</h2>
                      <p class="muted">
                        A curated, compatible set of client-side mods. Install
                        the full essentials collection or pick only what you
                        want. Nothing is installed until you confirm the plan.
                      </p>
                      <div class="chips">
                        <span class="chip accent">
                          {installedFeatureCount()} of {features().length}{" "}
                          installed
                        </span>
                        <span class="chip">
                          {loaderLabel(profile().loader)}
                        </span>
                        <span class="chip">MC {profile().mcVersion}</span>
                      </div>
                    </div>
                    <button
                      class="btn primary"
                      disabled={
                        !loaderReady() ||
                        planning() ||
                        essentialRemaining() === 0
                      }
                      onClick={installEssentials}
                    >
                      {planning()
                        ? "Resolving…"
                        : essentialRemaining()
                          ? `Install essentials (${essentialRemaining()})`
                          : "Essentials installed"}
                    </button>
                  </div>

                  <div class="filter-row">
                    <For each={categories()}>
                      {(category) => (
                        <button
                          class="filter"
                          classList={{
                            active: featureCategory() === category,
                          }}
                          onClick={() => setFeatureCategory(category)}
                        >
                          {category === "all" ? "All" : category}
                        </button>
                      )}
                    </For>
                  </div>

                  <Show when={visibleFeatures().length === 0}>
                    <div class="empty-state">
                      <h3>No features in this category</h3>
                      <p class="muted">Choose another category.</p>
                    </div>
                  </Show>

                  <div class="feature-grid">
                    <For each={visibleFeatures()}>
                      {(feature) => (
                        <article
                          class="feature-card"
                          classList={{
                            unavailable: !feature.available,
                            installed: !!feature.installed,
                          }}
                        >
                          <div class="feature-head">
                            <Show
                              when={feature.project?.iconUrl}
                              fallback={
                                <span class="mod-icon">
                                  {feature.name.slice(0, 1)}
                                </span>
                              }
                            >
                              <img
                                class="mod-icon"
                                src={feature.project!.iconUrl ?? ""}
                                alt=""
                                loading="lazy"
                              />
                            </Show>
                            <div class="feature-title">
                              <h3>{feature.name}</h3>
                              <span class="muted">
                                {feature.project?.author || "Kestrel"}
                              </span>
                            </div>
                            <Show when={feature.essential}>
                              <span class="badge accent">Essential</span>
                            </Show>
                          </div>
                          <p class="feature-description">
                            {feature.description}
                          </p>
                          <div class="chips">
                            <span class="chip">{feature.category}</span>
                            <span class="chip">Key: {feature.keybind}</span>
                            <Show when={feature.project?.license}>
                              <span class="chip">
                                {feature.project!.license}
                              </span>
                            </Show>
                            <Show when={feature.version}>
                              <span class="chip">
                                {feature.version!.versionNumber}
                              </span>
                            </Show>
                          </div>
                          <Show when={feature.reason && !feature.available}>
                            <p class="hint error">{feature.reason}</p>
                          </Show>
                          <div class="feature-actions">
                            <Show
                              when={feature.installed}
                              fallback={
                                <Show
                                  when={feature.available}
                                  fallback={
                                    <span class="status">Unavailable</span>
                                  }
                                >
                                  <button
                                    class="btn primary sm"
                                    disabled={!loaderReady() || planning()}
                                    onClick={() => installFeature(feature)}
                                  >
                                    Install
                                  </button>
                                </Show>
                              }
                            >
                              <span class="status ok">
                                Installed {feature.installed!.versionNumber}
                              </span>
                              <Show when={feature.update}>
                                <button
                                  class="btn ghost sm"
                                  disabled={!!installing()}
                                  onClick={() => updateFeature(feature)}
                                >
                                  Update to {feature.update!.latestVersion}
                                </button>
                              </Show>
                            </Show>
                            <Show when={feature.project}>
                              <button
                                class="link"
                                onClick={() =>
                                  setDetailId(feature.project!.id)
                                }
                              >
                                Details
                              </button>
                            </Show>
                          </div>
                        </article>
                      )}
                    </For>
                  </div>
                </div>
              </Show>

              <Show when={tab() === "browse"}>
                <div class="browse">
                  <div class="browse-toolbar">
                    <input
                      class="search"
                      type="search"
                      placeholder="Search Modrinth for mods…"
                      value={searchQuery()}
                      onInput={(event) =>
                        setSearchQuery(event.currentTarget.value)
                      }
                    />
                  </div>
                  <div class="filter-row">
                    <For each={BROWSE_CATEGORIES}>
                      {(category) => (
                        <button
                          class="filter"
                          classList={{
                            active: searchCategory() === category,
                          }}
                          onClick={() => setSearchCategory(category)}
                        >
                          {titleCase(category)}
                        </button>
                      )}
                    </For>
                  </div>

                  <Show when={!loaderReady()}>
                    <div class="empty-state">
                      <h3>Mod loader required</h3>
                      <p class="muted">{catalog()?.loaderError}</p>
                    </div>
                  </Show>

                  <Show when={searchError()}>
                    <p class="error">{searchError()}</p>
                  </Show>

                  <Show when={searching() && results().length === 0}>
                    <div class="mods-loading">
                      <span class="spinner" />
                      <p class="muted">Searching Modrinth…</p>
                    </div>
                  </Show>

                  <Show
                    when={
                      !searching() &&
                      loaderReady() &&
                      results().length === 0 &&
                      !searchError()
                    }
                  >
                    <div class="empty-state">
                      <h3>No mods found</h3>
                      <p class="muted">
                        Try a different search or category.
                      </p>
                    </div>
                  </Show>

                  <div class="mod-grid">
                    <For each={results()}>
                      {(mod) => (
                        <article
                          class="mod-card"
                          onClick={() => setDetailId(mod.id)}
                          tabIndex={0}
                          onKeyDown={(event) => {
                            if (event.key === "Enter" || event.key === " ") {
                              setDetailId(mod.id);
                            }
                          }}
                        >
                          <div class="mod-card-head">
                            <Show
                              when={mod.iconUrl}
                              fallback={
                                <span class="mod-icon">
                                  {mod.title.slice(0, 1)}
                                </span>
                              }
                            >
                              <img
                                class="mod-icon"
                                src={mod.iconUrl ?? ""}
                                alt=""
                                loading="lazy"
                              />
                            </Show>
                            <div class="mod-card-title">
                              <h3>{mod.title}</h3>
                              <span class="muted">
                                {mod.author || "unknown"}
                              </span>
                            </div>
                          </div>
                          <p class="mod-card-desc">{mod.description}</p>
                          <div class="chips">
                            <span class="chip accent">
                              {loaderLabel(profile().loader)}
                            </span>
                            <span class="chip">
                              {formatCount(mod.downloads)} downloads
                            </span>
                            <Show when={mod.license}>
                              <span class="chip">{mod.license}</span>
                            </Show>
                          </div>
                        </article>
                      )}
                    </For>
                  </div>

                  <Show when={results().length < searchTotal()}>
                    <button
                      class="btn ghost load-more"
                      disabled={searching()}
                      onClick={() => void runSearch(false)}
                    >
                      {searching()
                        ? "Loading…"
                        : `Load more (${searchTotal() - results().length} left)`}
                    </button>
                  </Show>
                </div>
              </Show>

              <Show when={tab() === "installed"}>
                <div class="installed">
                  <div class="section-row">
                    <div>
                      <h2>Installed mods</h2>
                      <p class="muted">
                        {installed().length} jar
                        {installed().length === 1 ? "" : "s"} in{" "}
                        <span class="mono">
                          {catalog()?.instance?.gameDir}/mods
                        </span>
                      </p>
                    </div>
                    <div class="actions">
                      <button
                        class="btn ghost"
                        disabled={busy() === "updates"}
                        onClick={() => void checkUpdates()}
                      >
                        {busy() === "updates"
                          ? "Checking…"
                          : "Check for updates"}
                      </button>
                    </div>
                  </div>

                  <Show
                    when={installed().length > 0}
                    fallback={
                      <div class="empty-state">
                        <h3>No mods installed</h3>
                        <p class="muted">
                          Install from Essentials or browse Modrinth.
                        </p>
                        <button
                          class="btn primary"
                          onClick={() => setTab("essentials")}
                        >
                          Browse essentials
                        </button>
                      </div>
                    }
                  >
                    <div class="installed-list">
                      <For each={installed()}>
                        {(mod) => {
                          const update = () =>
                            updates().find(
                              (entry) => entry.projectId === mod.projectId,
                            );
                          return (
                            <article
                              class="installed-row"
                              classList={{ disabled: !mod.enabled }}
                            >
                              <Show
                                when={mod.iconUrl}
                                fallback={
                                  <span class="mod-icon">
                                    {mod.title.slice(0, 1)}
                                  </span>
                                }
                              >
                                <img
                                  class="mod-icon"
                                  src={mod.iconUrl ?? ""}
                                  alt=""
                                  loading="lazy"
                                />
                              </Show>
                              <div class="installed-text">
                                <div class="installed-title">
                                  <strong>{mod.title}</strong>
                                  <Show when={update()}>
                                    <span class="badge accent">
                                      Update {update()!.latestVersion}
                                    </span>
                                  </Show>
                                </div>
                                <span class="muted">
                                  {mod.versionNumber || "local"} ·{" "}
                                  {mod.author || mod.source} ·{" "}
                                  {formatBytes(mod.size)}
                                </span>
                              </div>
                              <button
                                class="toggle"
                                classList={{ on: mod.enabled }}
                                disabled={busy() === mod.fileName}
                                aria-label={
                                  mod.enabled ? "Disable mod" : "Enable mod"
                                }
                                title={
                                  mod.enabled ? "Disable mod" : "Enable mod"
                                }
                                onClick={() => void toggleMod(mod)}
                              >
                                <span />
                              </button>
                              <button
                                class="btn ghost sm"
                                disabled={busy() === mod.fileName}
                                onClick={() => void removeMod(mod)}
                              >
                                Remove
                              </button>
                            </article>
                          );
                        }}
                      </For>
                    </div>
                  </Show>
                </div>
              </Show>

              <Show when={tab() === "updates"}>
                <div class="updates">
                  <div class="section-row">
                    <div>
                      <h2>Updates</h2>
                      <p class="muted">
                        Updates preserve your configs. Kestrel only installs
                        versions that support MC {profile().mcVersion} on{" "}
                        {loaderLabel(profile().loader)}.
                      </p>
                    </div>
                    <button
                      class="btn primary"
                      disabled={!updates().length || !!installing()}
                      onClick={() =>
                        void updateMods(
                          updates().map((update) => update.projectId),
                        )
                      }
                    >
                      Update all ({updates().length})
                    </button>
                  </div>

                  <Show
                    when={updates().length > 0}
                    fallback={
                      <div class="empty-state">
                        <h3>Everything is up to date</h3>
                        <p class="muted">
                          Checked against Modrinth for MC{" "}
                          {profile().mcVersion} on{" "}
                          {loaderLabel(profile().loader)}.
                        </p>
                        <button
                          class="btn ghost"
                          disabled={busy() === "updates"}
                          onClick={() => void checkUpdates()}
                        >
                          {busy() === "updates"
                            ? "Checking…"
                            : "Check again"}
                        </button>
                      </div>
                    }
                  >
                    <div class="installed-list">
                      <For each={updates()}>
                        {(update) => (
                          <article class="installed-row">
                            <Show
                              when={update.iconUrl}
                              fallback={
                                <span class="mod-icon">
                                  {update.title.slice(0, 1)}
                                </span>
                              }
                            >
                              <img
                                class="mod-icon"
                                src={update.iconUrl ?? ""}
                                alt=""
                                loading="lazy"
                              />
                            </Show>
                            <div class="installed-text">
                              <strong>{update.title}</strong>
                              <span class="muted">
                                {update.currentVersion} →{" "}
                                {update.latestVersion}
                              </span>
                            </div>
                            <button
                              class="btn primary sm"
                              disabled={!!installing()}
                              onClick={() =>
                                void updateMods([update.projectId])
                              }
                            >
                              Update
                            </button>
                          </article>
                        )}
                      </For>
                    </div>
                  </Show>
                </div>
              </Show>
            </Show>

            <Show when={pending()}>
              {(request) => (
                <div
                  class="modal-backdrop"
                  onClick={() => setPending(null)}
                >
                  <div
                    class="modal plan-modal"
                    role="dialog"
                    aria-modal="true"
                    aria-label="Install plan"
                    onClick={(event) => event.stopPropagation()}
                  >
                    <div class="modal-head">
                      <h2>Review install plan</h2>
                      <button
                        class="close"
                        aria-label="Close"
                        onClick={() => setPending(null)}
                      >
                        ×
                      </button>
                    </div>
                    <p class="muted">{request().label}</p>
                    <div class="plan-list">
                      <For each={request().plan.items}>
                        {(item) => (
                          <div class="plan-row">
                            <Show
                              when={item.iconUrl}
                              fallback={
                                <span class="mod-icon sm">
                                  {item.title.slice(0, 1)}
                                </span>
                              }
                            >
                              <img
                                class="mod-icon sm"
                                src={item.iconUrl ?? ""}
                                alt=""
                                loading="lazy"
                              />
                            </Show>
                            <div class="plan-text">
                              <strong>{item.title}</strong>
                              <span class="muted">
                                {item.versionNumber} ·{" "}
                                {item.license || "license unknown"}
                              </span>
                            </div>
                            <Show when={item.isDependency}>
                              <span class="badge">Dependency</span>
                            </Show>
                            <span class="hint mono">
                              {formatBytes(item.size)}
                            </span>
                          </div>
                        )}
                      </For>
                    </div>
                    <Show when={request().plan.warnings.length}>
                      <div class="plan-warnings">
                        <For each={request().plan.warnings}>
                          {(warning) => <p class="hint">{warning}</p>}
                        </For>
                      </div>
                    </Show>
                    <div class="plan-summary">
                      <span class="muted">
                        {request().plan.items.length} file
                        {request().plan.items.length === 1 ? "" : "s"} ·{" "}
                        {formatBytes(
                          request().plan.items.reduce(
                            (sum, item) => sum + item.size,
                            0,
                          ),
                        )}
                      </span>
                      <div class="actions">
                        <button
                          class="btn ghost"
                          onClick={() => setPending(null)}
                        >
                          Cancel
                        </button>
                        <button
                          class="btn primary"
                          onClick={() => void confirmInstall()}
                        >
                          Install {request().plan.items.length} mod
                          {request().plan.items.length === 1 ? "" : "s"}
                        </button>
                      </div>
                    </div>
                  </div>
                </div>
              )}
            </Show>

            <Show when={installing()}>
              <div class="install-overlay">
                <div class="install-panel">
                  <span class="spinner big" />
                  <h3>{phaseLabel(progress()?.phase)}</h3>
                  <p class="muted">{installing()}</p>
                  <div class="progress">
                    <div
                      class="progress-fill"
                      style={{ width: `${percent()}%` }}
                    />
                  </div>
                  <div class="progress-meta">
                    <span class="muted">{progressText()}</span>
                    <span class="muted">{percent()}%</span>
                  </div>
                  <Show when={progress()?.current}>
                    <p class="hint mono ellipsis">{progress()!.current}</p>
                  </Show>
                  <p class="hint">
                    Files are verified before they enter mods/. Incomplete
                    installs roll back automatically.
                  </p>
                </div>
              </div>
            </Show>

            <Show when={detailId()}>
              {(id) => (
                <ModDetailModal
                  instanceId={profile().id}
                  projectId={id()}
                  installed={detailInstalled()}
                  update={detailUpdate()}
                  loaderReady={loaderReady()}
                  loaderError={catalog()?.loaderError ?? null}
                  onClose={() => setDetailId(null)}
                  onInstall={(items, label) => {
                    setDetailId(null);
                    void planInstall(items, label);
                  }}
                />
              )}
            </Show>
          </>
        )}
      </Show>
    </section>
  );
}
