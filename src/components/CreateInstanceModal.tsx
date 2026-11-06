import {
  createEffect,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
} from "solid-js";

import { api } from "../lib/ipc";
import type { AppState, Loader, LoaderVersion, McVersion } from "../lib/types";
import { loaderLabel } from "../lib/format";

interface Props {
  onClose: () => void;
  onCreated: (state: AppState, warning: string | null) => void;
  onNotice: (message: string) => void;
}

const LOADERS: Loader[] = ["vanilla", "fabric", "quilt"];

export default function CreateInstanceModal(props: Props) {
  const [name, setName] = createSignal("");
  const [mcVersion, setMcVersion] = createSignal("");
  const [loader, setLoader] = createSignal<Loader>("fabric");
  const [loaderVersion, setLoaderVersion] = createSignal("");
  const [mcVersions, setMcVersions] = createSignal<McVersion[]>([]);
  const [loaderVersions, setLoaderVersions] = createSignal<LoaderVersion[]>([]);
  const [loadingVersions, setLoadingVersions] = createSignal(false);
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  onMount(async () => {
    try {
      const versions = await api.listMcVersions();
      setMcVersions(versions);
      if (versions.length) setMcVersion(versions[0].id);
    } catch (err) {
      setError(String(err));
    }
  });

  createEffect(() => {
    const selected = loader();
    const mc = mcVersion();
    setLoaderVersion("");
    setLoaderVersions([]);
    if (!mc || (selected !== "fabric" && selected !== "quilt")) return;

    let cancelled = false;
    setLoadingVersions(true);
    void (async () => {
      try {
        const versions = await api.listLoaderVersions(selected, mc);
        if (!cancelled) setLoaderVersions(versions);
      } catch (err) {
        if (!cancelled) setError(String(err));
      } finally {
        if (!cancelled) setLoadingVersions(false);
      }
    })();
    onCleanup(() => {
      cancelled = true;
    });
  });

  async function submit(event: Event) {
    event.preventDefault();
    if (submitting()) return;
    const trimmed = name().trim();
    if (!trimmed) {
      setError("Give the instance a name.");
      return;
    }
    if (!mcVersion()) {
      setError("Choose a Minecraft version.");
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      const result = await api.createInstance(
        trimmed,
        mcVersion(),
        loader(),
        loaderVersion() || null,
      );
      props.onCreated(result.state, result.warning);
      if (result.warning) props.onNotice(result.warning);
      props.onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(false);
    }
  }

  const needsLoader = () => loader() === "fabric" || loader() === "quilt";

  return (
    <div class="modal-backdrop" onClick={props.onClose}>
      <form
        class="modal create-instance"
        role="dialog"
        aria-modal="true"
        aria-label="Create instance"
        onClick={(event) => event.stopPropagation()}
        onSubmit={(event) => void submit(event)}
      >
        <div class="modal-head">
          <h2>New instance</h2>
          <button
            type="button"
            class="close"
            aria-label="Close"
            onClick={props.onClose}
          >
            ×
          </button>
        </div>

        <p class="muted create-intro">
          An instance is an isolated game directory with its own mods, configs
          and worlds.
        </p>

        <label class="field">
          <span class="label">Name</span>
          <input
            type="text"
            value={name()}
            maxlength={48}
            placeholder="My Fabric world"
            autofocus
            onInput={(event) => setName(event.currentTarget.value)}
          />
        </label>

        <label class="field">
          <span class="label">Minecraft version</span>
          <select
            value={mcVersion()}
            onChange={(event) => setMcVersion(event.currentTarget.value)}
          >
            <Show when={!mcVersions().length}>
              <option value="">Loading versions…</option>
            </Show>
            <For each={mcVersions()}>
              {(version) => <option value={version.id}>{version.id}</option>}
            </For>
          </select>
        </label>

        <div class="field">
          <span class="label">Mod loader</span>
          <div class="segmented">
            <For each={LOADERS}>
              {(option) => (
                <button
                  type="button"
                  class="seg"
                  classList={{ active: loader() === option }}
                  onClick={() => setLoader(option)}
                >
                  {loaderLabel(option)}
                </button>
              )}
            </For>
            <button type="button" class="seg" disabled title="Coming next">
              Forge
            </button>
            <button type="button" class="seg" disabled title="Coming next">
              NeoForge
            </button>
          </div>
        </div>

        <Show when={needsLoader()}>
          <label class="field">
            <span class="label">Loader version</span>
            <select
              value={loaderVersion()}
              disabled={loadingVersions()}
              onChange={(event) => setLoaderVersion(event.currentTarget.value)}
            >
              <option value="">
                {loadingVersions() ? "Loading loader versions…" : "Latest stable"}
              </option>
              <For each={loaderVersions()}>
                {(version) => (
                  <option value={version.version}>
                    {version.version}
                    {version.stable ? "" : " (beta)"}
                  </option>
                )}
              </For>
            </select>
            <span class="hint">
              Downloaded from the official {loaderLabel(loader())} meta server
              and verified against its hashes.
            </span>
          </label>
        </Show>

        <Show when={!needsLoader()}>
          <p class="hint">
            Vanilla instances have no mod loader. You can still browse and
            import mods, but they will not load in game.
          </p>
        </Show>

        <Show when={error()}>
          <p class="error">{error()}</p>
        </Show>

        <div class="modal-actions">
          <button type="button" class="btn ghost" onClick={props.onClose}>
            Cancel
          </button>
          <button
            type="submit"
            class="btn primary"
            disabled={submitting() || !mcVersion()}
          >
            {submitting()
              ? needsLoader()
                ? "Creating and installing loader…"
                : "Creating…"
              : "Create instance"}
          </button>
        </div>
      </form>
    </div>
  );
}
