import { createMemo, createSignal, For, onMount, Show } from "solid-js";

import { api } from "./lib/ipc";
import type { AppState, Bootstrap } from "./lib/types";

const PRESET_LABEL: Record<string, string> = {
  optimized: "Optimized",
  vanilla: "Vanilla",
  custom: "Custom",
};

const LOADER_LABEL: Record<string, string> = {
  vanilla: "Vanilla",
  fabric: "Fabric",
  quilt: "Quilt",
  neoforge: "NeoForge",
  forge: "Forge",
};

function ago(ms: number | null): string {
  if (!ms) return "never";
  const diff = Date.now() - ms;
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins} min ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours} h ago`;
  return `${Math.floor(hours / 24)} d ago`;
}

export default function App() {
  const [boot, setBoot] = createSignal<Bootstrap | null>(null);
  const [state, setState] = createSignal<AppState | null>(null);
  const [startupMs, setStartupMs] = createSignal<number | null>(null);
  const [notice, setNotice] = createSignal<string | null>(null);

  onMount(async () => {
    void api.logTiming("js_mounted", performance.now(), "ui").catch(() => {});
    try {
      const t = await api.bootstrap();
      setBoot(t);
      setState(t.state);

      // Honour `kestrel --profile NAME`.
      const wanted = t.launch.profile;
      if (wanted) {
        const match = t.state.profiles.find(
          (p) =>
            p.id === wanted ||
            p.name.toLowerCase() === wanted.toLowerCase(),
        );
        if (match) await select(match.id);
      }

      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          const total = Date.now() - t.processStartEpochMs;
          setStartupMs(total);
          void api.logTiming("first_paint", total, "ui");
        }),
      );

      await api.onBackgroundIdle(() => {});
    } catch (err) {
      setNotice(String(err));
      void api
        .logTiming("bootstrap_error", performance.now(), String(err))
        .catch(() => {});
    }
  });

  const profiles = () => state()?.profiles ?? [];
  const accounts = () => state()?.accounts ?? [];
  const selected = createMemo(() => {
    const s = state();
    if (!s) return null;
    return (
      s.profiles.find((p) => p.id === s.selectedProfile) ??
      s.profiles[0] ??
      null
    );
  });

  async function select(id: string) {
    const s = state();
    if (!s) return;
    const next: AppState = { ...s, selectedProfile: id };
    setState(next);
    try {
      await api.saveState(next);
    } catch (err) {
      setNotice(String(err));
    }
  }

  return (
    <div class="app">
      <aside class="sidebar">
        <div class="brand">
          <span class="brand-mark">K</span>
          <span class="brand-name">
            KEST<span>R</span>EL
          </span>
        </div>

        <div class="section-head">
          <span>Profiles</span>
          <span class="count">{profiles().length}</span>
        </div>

        <div class="profile-list">
          <For each={profiles()}>
            {(profile) => (
              <button
                class="profile"
                classList={{ active: selected()?.id === profile.id }}
                onClick={() => void select(profile.id)}
                title={profile.name}
              >
                <span class="profile-avatar">
                  {profile.name.slice(0, 1).toUpperCase()}
                </span>
                <span class="profile-text">
                  <span class="profile-name">{profile.name}</span>
                  <span class="profile-sub">
                    {profile.mcVersion || "no version"} ·{" "}
                    {LOADER_LABEL[profile.loader] ?? profile.loader}
                  </span>
                </span>
              </button>
            )}
          </For>

          <Show when={profiles().length === 0}>
            <div class="empty">
              <p>No profiles yet.</p>
              <p class="muted">Profile creation arrives in stage 8.</p>
            </div>
          </Show>
        </div>

        <div class="sidebar-foot">
          <div class="account">
            <span class="dot" />
            <Show
              when={accounts().length > 0}
              fallback={<span class="muted">No account</span>}
            >
              <span>{accounts()[0].username}</span>
            </Show>
          </div>
          <div class="version">v{boot()?.appVersion ?? "0.1.0"}</div>
        </div>
      </aside>

      <main class="main">
        <header class="topbar">
          <div class="titles">
            <h1>{selected()?.name ?? "Welcome to Kestrel"}</h1>
            <p class="muted">
              {selected()
                ? `${selected()!.mcVersion} · ${LOADER_LABEL[selected()!.loader]}`
                : "A fast, lightweight launcher for Minecraft: Java Edition"}
            </p>
          </div>
          <div class="actions">
            <button class="btn ghost" disabled title="Stage 8">
              New profile
            </button>
            <button
              class="btn primary"
              disabled
              title="Launching lands in stage 2"
            >
              Launch
            </button>
          </div>
        </header>

        <Show
          when={selected()}
          fallback={
            <section class="cards">
              <div class="card wide">
                <h2>Built for instant starts</h2>
                <p class="muted">
                  Kestrel paints from a local cache before any network call and
                  keeps the game launch on a prebuilt classpath. Create a
                  profile to get going.
                </p>
              </div>
            </section>
          }
        >
          {(profile) => (
            <section class="cards">
              <div class="card">
                <span class="label">Preset</span>
                <span class="value">
                  {PRESET_LABEL[profile().preset] ?? profile().preset}
                </span>
                <span class="hint">
                  {profile().preset === "optimized"
                    ? "Sodium, Lithium, Iris and friends"
                    : "No content preset"}
                </span>
              </div>
              <div class="card">
                <span class="label">Memory</span>
                <span class="value">
                  {profile().memoryMb
                    ? `${profile().memoryMb} MB`
                    : "Automatic"}
                </span>
                <span class="hint">-Xms equals -Xmx at launch</span>
              </div>
              <div class="card">
                <span class="label">Java</span>
                <span class="value">
                  {profile().javaPath ? "Custom" : "Automatic"}
                </span>
                <span class="hint">Stage 4 picks the right JDK</span>
              </div>
              <div class="card">
                <span class="label">Last played</span>
                <span class="value">{ago(profile().lastPlayed)}</span>
                <span class="hint">
                  {profile().loaderVersion
                    ? `Loader ${profile().loaderVersion}`
                    : "Loader not pinned"}
                </span>
              </div>
            </section>
          )}
        </Show>

        <section class="perf">
          <div class="perf-item">
            <span class="label">Cold start</span>
            <span class="perf-value">
              {startupMs() === null ? "…" : `${startupMs()} ms`}
            </span>
            <span class="hint">process start → interactive UI</span>
          </div>
          <div class="perf-item">
            <span class="label">Backend ready</span>
            <span class="perf-value">
              {boot() ? `${boot()!.backendReadyMs.toFixed(1)} ms` : "…"}
            </span>
            <span class="hint">cache + config load</span>
          </div>
          <div class="perf-item">
            <span class="label">Timing log</span>
            <span class="perf-value mono">logs/timings.jsonl</span>
            <span class="hint">every phase is recorded</span>
          </div>
        </section>

        <footer class="legal">{boot()?.legal ?? ""}</footer>

        <Show when={notice()}>
          <div class="toast" onClick={() => setNotice(null)}>
            {notice()}
          </div>
        </Show>
      </main>
    </div>
  );
}
