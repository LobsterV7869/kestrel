import {
  createMemo,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { api } from "./lib/ipc";
import type {
  AccountSummary,
  AppState,
  Bootstrap,
  DeviceCodeInfo,
} from "./lib/types";
import CreateInstanceModal from "./components/CreateInstanceModal";
import ModsPage from "./components/ModsPage";

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

type SignInStatus =
  | "idle"
  | "starting"
  | "waiting"
  | "expired"
  | "declined"
  | "error";

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
  const [view, setView] = createSignal<"home" | "mods">("home");
  const [instanceOpen, setInstanceOpen] = createSignal(false);

  const [accountOpen, setAccountOpen] = createSignal(false);
  const [device, setDevice] = createSignal<DeviceCodeInfo | null>(null);
  const [signInStatus, setSignInStatus] = createSignal<SignInStatus>("idle");
  const [authError, setAuthError] = createSignal<string | null>(null);
  const [copied, setCopied] = createSignal(false);

  let pollGeneration = 0;
  let pollTimer: number | undefined;
  let pollIntervalMs = 5000;
  let unlistenAccount: UnlistenFn | undefined;
  let unlistenAuthError: UnlistenFn | undefined;

  onMount(async () => {
    void api.logTiming("js_mounted", performance.now(), "ui").catch(() => {});
    try {
      unlistenAccount = await api
        .onAccount((account) => applyAccount(account))
        .catch(() => undefined);
      unlistenAuthError = await api
        .onAuthError((message) => setNotice(message))
        .catch(() => undefined);

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
      void refreshAccount();
    } catch (err) {
      setNotice(String(err));
      void api
        .logTiming("bootstrap_error", performance.now(), String(err))
        .catch(() => {});
    }
  });

  onCleanup(() => {
    pollGeneration++;
    if (pollTimer) clearTimeout(pollTimer);
    unlistenAccount?.();
    unlistenAuthError?.();
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

  function applyAccount(account: AccountSummary | null) {
    const s = state();
    if (!s) return;
    setState({ ...s, accounts: account ? [account] : [] });
  }

  async function refreshAccount() {
    try {
      const account = await api.authRestore();
      if (account) applyAccount(account);
    } catch {
      // The backend emits kestrel://auth-error with the user-facing message.
    }
  }

  function stopPolling() {
    pollGeneration++;
    if (pollTimer) {
      clearTimeout(pollTimer);
      pollTimer = undefined;
    }
  }

  function closeAccount() {
    stopPolling();
    setAccountOpen(false);
    setDevice(null);
    setSignInStatus("idle");
    setAuthError(null);
  }

  async function startSignIn() {
    if (!boot()?.azureConfigured) {
      setAuthError(
        "Microsoft sign-in is not configured. Set azureClientId in ~/.config/kestrel/config.json.",
      );
      setSignInStatus("error");
      return;
    }
    stopPolling();
    setDevice(null);
    setAuthError(null);
    setCopied(false);
    setSignInStatus("starting");
    try {
      const info = await api.authStart();
      setDevice(info);
      setSignInStatus("waiting");
      pollIntervalMs = Math.max(info.interval, 5) * 1000;
      schedulePoll(pollGeneration, pollIntervalMs);
    } catch (err) {
      setAuthError(String(err));
      setSignInStatus("error");
      setDevice(null);
    }
  }

  function schedulePoll(generation: number, delayMs: number) {
    if (pollTimer) clearTimeout(pollTimer);
    pollTimer = window.setTimeout(() => void pollOnce(generation), delayMs);
  }

  async function pollOnce(generation: number) {
    if (generation !== pollGeneration) return;
    try {
      const result = await api.authPoll();
      if (generation !== pollGeneration) return;
      switch (result.status) {
        case "pending":
          setSignInStatus("waiting");
          schedulePoll(generation, pollIntervalMs);
          break;
        case "slowDown":
          pollIntervalMs += 5000;
          setSignInStatus("waiting");
          schedulePoll(generation, pollIntervalMs);
          break;
        case "success":
          stopPolling();
          if (result.account) applyAccount(result.account);
          setDevice(null);
          setSignInStatus("idle");
          setAccountOpen(false);
          setNotice(
            `Signed in as ${result.account?.username ?? "your account"}.`,
          );
          break;
        case "expired":
          stopPolling();
          setDevice(null);
          setSignInStatus("expired");
          break;
        case "declined":
          stopPolling();
          setDevice(null);
          setSignInStatus("declined");
          break;
      }
    } catch (err) {
      if (generation !== pollGeneration) return;
      stopPolling();
      setAuthError(String(err));
      setSignInStatus("error");
    }
  }

  async function signOut() {
    try {
      await api.authLogout();
      applyAccount(null);
      setNotice("Signed out.");
      closeAccount();
    } catch (err) {
      setAuthError(String(err));
    }
  }

  async function copyCode() {
    const current = device();
    if (!current) return;
    try {
      await navigator.clipboard.writeText(current.userCode);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      setNotice("Couldn't copy the code; select it manually.");
    }
  }

  function openAccount() {
    setAuthError(null);
    setSignInStatus("idle");
    setDevice(null);
    setAccountOpen(true);
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

        <nav class="side-nav" aria-label="Sections">
          <button
            class="side-nav-item"
            classList={{ active: view() === "home" }}
            onClick={() => setView("home")}
          >
            <span class="side-nav-icon">◈</span>
            <span>Play</span>
          </button>
          <button
            class="side-nav-item"
            classList={{ active: view() === "mods" }}
            onClick={() => setView("mods")}
          >
            <span class="side-nav-icon">▦</span>
            <span>Mods</span>
            <Show when={profiles().length > 0}>
              <span class="side-nav-badge">K</span>
            </Show>
          </button>
        </nav>

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
              <p>No instances yet.</p>
              <p class="muted">Create a Fabric or Quilt instance to begin.</p>
              <button
                class="btn primary sm"
                onClick={() => setInstanceOpen(true)}
              >
                Create instance
              </button>
            </div>
          </Show>
        </div>

        <div class="sidebar-foot">
          <button class="account" onClick={openAccount} title="Microsoft account">
            <span class="dot" classList={{ online: accounts().length > 0 }} />
            <Show
              when={accounts().length > 0}
              fallback={<span class="muted">Sign in</span>}
            >
              <span>{accounts()[0].username}</span>
            </Show>
          </button>
          <div class="version">v{boot()?.appVersion ?? "0.1.0"}</div>
        </div>
      </aside>

      <main class="main">
        <Show
          when={view() === "home"}
          fallback={
            <ModsPage
              profile={selected()}
              onStateChange={(next) => setState(next)}
              onCreateInstance={() => setInstanceOpen(true)}
              onNotice={(message) => setNotice(message)}
            />
          }
        >
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
            <Show when={accounts().length === 0}>
              <button class="btn primary" onClick={openAccount}>
                Sign in
              </button>
            </Show>
            <button class="btn ghost" onClick={() => setInstanceOpen(true)}>
              New instance
            </button>
            <button
              class="btn primary"
              disabled
              title="Launching from the UI is not part of this build yet"
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
                  keeps the game launch on a prebuilt classpath. Create an
                  instance to get going.
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
        </Show>

        <Show when={instanceOpen()}>
          <CreateInstanceModal
            onClose={() => setInstanceOpen(false)}
            onCreated={(next, warning) => {
              setState(next);
              if (warning) setNotice(warning);
            }}
            onNotice={(message) => setNotice(message)}
          />
        </Show>

        <Show when={accountOpen()}>
          <div class="modal-backdrop" onClick={closeAccount}>
            <div
              class="modal"
              role="dialog"
              aria-modal="true"
              aria-label="Microsoft account"
              onClick={(event) => event.stopPropagation()}
            >
              <div class="modal-head">
                <h2>Microsoft account</h2>
                <button
                  class="close"
                  aria-label="Close"
                  onClick={closeAccount}
                >
                  ×
                </button>
              </div>

              <Show
                when={accounts().length > 0}
                fallback={
                  <div class="signin">
                    <p class="muted">
                      Sign in to play Minecraft: Java Edition. Your password
                      never touches Kestrel — you approve a short code in your
                      browser.
                    </p>

                    <Show
                      when={device()}
                      fallback={
                        <>
                          <Show when={authError()}>
                            <p class="error">{authError()}</p>
                          </Show>
                          <Show when={signInStatus() === "expired"}>
                            <p class="error">
                              That code expired. Start again to get a fresh
                              one.
                            </p>
                          </Show>
                          <Show when={signInStatus() === "declined"}>
                            <p class="error">Sign-in was declined.</p>
                          </Show>
                          <Show when={!boot()?.azureConfigured}>
                            <p class="error">
                              Sign-in is not configured. Set{" "}
                              <code>azureClientId</code> in{" "}
                              <code>~/.config/kestrel/config.json</code> (public
                              client with the device-code flow enabled).
                            </p>
                          </Show>
                          <button
                            class="btn primary"
                            disabled={
                              !boot()?.azureConfigured ||
                              signInStatus() === "starting"
                            }
                            onClick={() => void startSignIn()}
                          >
                            {signInStatus() === "starting"
                              ? "Contacting Microsoft…"
                              : "Continue with Microsoft"}
                          </button>
                        </>
                      }
                    >
                      {(code) => (
                        <div class="device">
                          <p class="muted">{code().message}</p>
                          <div class="device-row">
                            <span class="device-code">{code().userCode}</span>
                            <button
                              class="btn ghost"
                              onClick={() => void copyCode()}
                            >
                              {copied() ? "Copied" : "Copy"}
                            </button>
                          </div>
                          <button
                            class="link"
                            onClick={() =>
                              void api.openExternal(code().verificationUri)
                            }
                          >
                            Open{" "}
                            {code().verificationUri.replace(
                              /^https?:\/\//,
                              "",
                            )}
                          </button>
                          <p class="hint">
                            {signInStatus() === "error" && authError()
                              ? authError()
                              : "Waiting for approval… this window updates automatically."}
                          </p>
                          <Show
                            when={signInStatus() === "error"}
                            fallback={
                              <button
                                class="btn ghost"
                                onClick={closeAccount}
                              >
                                Cancel
                              </button>
                            }
                          >
                            <button
                              class="btn primary"
                              onClick={() => void startSignIn()}
                            >
                              Try again
                            </button>
                          </Show>
                        </div>
                      )}
                    </Show>
                  </div>
                }
              >
                <div class="signed-in">
                  <div class="account-big">
                    <span class="profile-avatar big">
                      {accounts()[0].username.slice(0, 1).toUpperCase()}
                    </span>
                    <div>
                      <strong>{accounts()[0].username}</strong>
                      <div class="mono muted">{accounts()[0].uuid}</div>
                    </div>
                  </div>
                  <p class="muted">
                    The session lives in the system keyring and is refreshed
                    silently on startup.
                  </p>
                  <button class="btn ghost" onClick={() => void signOut()}>
                    Sign out
                  </button>
                </div>
              </Show>
            </div>
          </div>
        </Show>

        <Show when={notice()}>
          <div class="toast" onClick={() => setNotice(null)}>
            {notice()}
          </div>
        </Show>
      </main>
    </div>
  );
}

