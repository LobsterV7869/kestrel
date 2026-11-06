import { createMemo, createSignal, For, onMount, Show } from "solid-js";

import { api } from "../lib/ipc";
import type {
  InstallItem,
  InstalledMod,
  ModUpdate,
  ModVersion,
  ProjectDetail,
} from "../lib/types";
import { formatCount, relativeTime, titleCase } from "../lib/format";

interface Props {
  instanceId: string;
  projectId: string;
  installed: InstalledMod | null;
  update: ModUpdate | null;
  loaderReady: boolean;
  loaderError: string | null;
  onClose: () => void;
  onInstall: (items: InstallItem[], label: string) => void;
}

export default function ModDetailModal(props: Props) {
  const [detail, setDetail] = createSignal<ProjectDetail | null>(null);
  const [versions, setVersions] = createSignal<ModVersion[]>([]);
  const [selectedVersion, setSelectedVersion] = createSignal<string>("");
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal<string | null>(null);

  onMount(async () => {
    try {
      const [project, list] = await Promise.all([
        api.modsProject(props.projectId),
        api.modsVersions(props.instanceId, props.projectId).catch(() => []),
      ]);
      setDetail(project);
      setVersions(list);
      if (list.length) setSelectedVersion(list[0].id);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  });

  const chosen = createMemo(() => {
    const id = selectedVersion();
    return versions().find((version) => version.id === id) ?? versions()[0] ?? null;
  });

  const gallery = createMemo(() => detail()?.gallery?.slice(0, 3) ?? []);
  const canInstall = createMemo(
    () => props.loaderReady && !loading() && !error() && versions().length > 0,
  );

  function install() {
    const project = detail();
    if (!project) return;
    const version = chosen();
    props.onInstall(
      [
        {
          projectId: project.id,
          versionId: version?.id ?? null,
          featureId: props.installed?.featureId ?? null,
        },
      ],
      `${props.installed ? "Updating" : "Installing"} ${project.title}`,
    );
  }

  return (
    <div class="modal-backdrop" onClick={props.onClose}>
      <div
        class="modal mod-detail"
        role="dialog"
        aria-modal="true"
        aria-label="Mod details"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="modal-head">
          <h2>{detail()?.title ?? "Mod details"}</h2>
          <button class="close" aria-label="Close" onClick={props.onClose}>
            ×
          </button>
        </div>

        <Show when={loading()}>
          <div class="detail-loading">
            <span class="spinner" />
            <p class="muted">Loading project details…</p>
          </div>
        </Show>

        <Show when={error()}>
          <p class="error">{error()}</p>
        </Show>

        <Show when={detail()}>
          {(project) => (
            <div class="detail">
              <div class="detail-hero">
                <Show
                  when={project().iconUrl}
                  fallback={
                    <span class="mod-icon xl">
                      {project().title.slice(0, 1).toUpperCase()}
                    </span>
                  }
                >
                  <img
                    class="mod-icon xl"
                    src={project().iconUrl ?? ""}
                    alt=""
                    loading="lazy"
                  />
                </Show>
                <div class="detail-hero-text">
                  <div class="detail-title-row">
                    <h3>{project().title}</h3>
                    <span class="badge">{project().license || "Unknown license"}</span>
                  </div>
                  <p class="muted">
                    by {project().author || "unknown"} ·{" "}
                    {formatCount(project().downloads)} downloads
                  </p>
                  <p class="detail-description">{project().description}</p>
                  <div class="chips">
                    <For each={project().loaders}>
                      {(loader) => (
                        <span class="chip accent">{titleCase(loader)}</span>
                      )}
                    </For>
                    <Show when={project().clientSide === "required"}>
                      <span class="chip">Client required</span>
                    </Show>
                    <Show when={project().sourceUrl}>
                      <button
                        class="chip link-chip"
                        onClick={() =>
                          void api.openExternal(project().sourceUrl ?? "")
                        }
                      >
                        Source
                      </button>
                    </Show>
                  </div>
                </div>
              </div>

              <Show when={gallery().length}>
                <div class="gallery">
                  <For each={gallery()}>
                    {(image) => (
                      <figure class="gallery-item">
                        <img src={image.url} alt={image.title ?? ""} loading="lazy" />
                        <Show when={image.title}>
                          <figcaption>{image.title}</figcaption>
                        </Show>
                      </figure>
                    )}
                  </For>
                </div>
              </Show>

              <div class="detail-grid">
                <div class="detail-section">
                  <span class="label">Compatible versions</span>
                  <p class="detail-value">
                    {project().gameVersions.length
                      ? `${project().gameVersions.length} Minecraft versions`
                      : "Not listed"}
                  </p>
                  <p class="hint">
                    {project().gameVersions.slice(-8).join(", ") || "—"}
                  </p>
                </div>
                <div class="detail-section">
                  <span class="label">Loaders</span>
                  <p class="detail-value">
                    {project().loaders.map(titleCase).join(", ") || "—"}
                  </p>
                </div>
                <div class="detail-section">
                  <span class="label">Latest compatible</span>
                  <p class="detail-value">
                    {chosen()?.versionNumber ?? "None for this instance"}
                  </p>
                  <Show when={chosen()}>
                    <p class="hint">
                      {titleCase(chosen()!.versionType)} ·{" "}
                      {relativeTime(chosen()!.datePublished)}
                    </p>
                  </Show>
                </div>
                <div class="detail-section">
                  <span class="label">Status</span>
                  <p class="detail-value">
                    <Show
                      when={props.installed}
                      fallback={
                        <span class="status">Not installed</span>
                      }
                    >
                      <span class="status ok">
                        Installed {props.installed?.versionNumber}
                      </span>
                    </Show>
                  </p>
                  <Show when={props.update}>
                    <p class="hint accent-text">
                      Update available: {props.update!.latestVersion}
                    </p>
                  </Show>
                </div>
              </div>

              <Show when={versions().length > 1}>
                <label class="field">
                  <span class="label">Version</span>
                  <select
                    value={selectedVersion()}
                    onChange={(event) =>
                      setSelectedVersion(event.currentTarget.value)
                    }
                  >
                    <For each={versions()}>
                      {(version) => (
                        <option value={version.id}>
                          {version.versionNumber} · {titleCase(version.versionType)}
                        </option>
                      )}
                    </For>
                  </select>
                </label>
              </Show>

              <Show when={project().body}>
                <div class="detail-body">
                  <span class="label">About this mod</span>
                  <div class="mod-body">{project().body}</div>
                </div>
              </Show>

              <div class="detail-actions">
                <Show when={!props.loaderReady}>
                  <p class="hint error">{props.loaderError}</p>
                </Show>
                <Show when={props.loaderReady && !versions().length && !loading()}>
                  <p class="hint error">
                    No compatible version exists for this instance.
                  </p>
                </Show>
                <button class="btn ghost" onClick={props.onClose}>
                  Close
                </button>
                <button
                  class="btn primary"
                  disabled={!canInstall()}
                  onClick={install}
                >
                  {props.installed
                    ? props.update
                      ? "Update"
                      : "Reinstall"
                    : "Install"}
                </button>
              </div>
            </div>
          )}
        </Show>
      </div>
    </div>
  );
}
