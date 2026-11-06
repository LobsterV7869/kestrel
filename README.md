# Kestrel

A fast, lightweight Minecraft: Java Edition launcher for Linux. Rust + Tauri 2
backend, a tiny SolidJS frontend, and no Electron.

> **Not an official Minecraft product. Not approved by or associated with
> Mojang or Microsoft.**

Kestrel is licensed under **GPL-3.0-or-later**. It never bundles or
redistributes Mojang/Microsoft files: the client, libraries, assets and Java
runtimes are downloaded from official servers at install time. It contains no
offline/cracked-account support.

## Why "Kestrel"?

A kestrel is a small, fast falcon — a good fit for a launcher whose main goal is
speed. The name is original and does not use Mojang's trademarks. The icon is an
original geometric mark.

## Status

Built in stages; see `PERFORMANCE.md` for measured numbers.

- [x] Stage 1 — Tauri scaffold, instant cache-first UI, `bench` harness (see PERFORMANCE.md)
- [x] Stage 2 — Version manifest, downloads, shared cache, vanilla launch
- [x] Stage 3 — Microsoft auth (device code) + keyring
- [ ] Stage 4 — Java runtime management
- [ ] Stage 5 — Fabric, Modrinth browser, Optimized preset
- [ ] Stage 6 — Performance work (pre-warm, AppCDS, JVM presets)
- [ ] Stage 7 — Quick Play, server list, join flow
- [ ] Stage 8 — Profiles, import/export, console, crash view
- [ ] Stage 9 — Packaging, CI, docs, licensing polish

## Requirements

- Linux with a Wayland or X11 session
- Rust 1.85+ (2021 edition) and Cargo
- Node.js 20+ and npm
- Tauri system dependencies: `webkit2gtk-4.1`, `gtk3`, `libsoup3`, `librsvg`
  (on Arch: `pacman -S webkit2gtk-4.1 gtk3 libsoup3 librsvg`)

## Build and run

```bash
npm install
npm run tauri dev      # development (Vite dev server + hot reload)
```

Production build (embedded frontend, no dev server needed):

```bash
npm run build
cargo build --release -p kestrel-app --features custom-protocol
./target/release/kestrel
```

> A plain `cargo build` (debug) loads `devUrl` (Vite on :1420), so run
> `npm run dev` alongside it. Release with the `custom-protocol` feature embeds
> `dist/`. `npm run tauri build` does both automatically and produces the
> AppImage/.deb (stage 9).

Benchmarks and headless commands:

```bash
cargo build --release -p kestrel-app --features custom-protocol
./target/release/kestrel bench
./target/release/kestrel install 1.20.4     # warm the shared cache
KESTREL_DEV_AUTH=1 ./target/release/kestrel dev-launch 1.20.4
```

`dev-launch` is a development smoke test: it uses a placeholder token (never
persisted, disabled unless `KESTREL_DEV_AUTH=1`) and launches the vanilla title
screen. It is not an offline-mode feature. Sign in with a real Microsoft account
from the launcher UI instead.

### Layout

```
~/.local/share/kestrel/
  shared/versions/<id>/          version JSON, client jar, extracted natives
  shared/libraries/              Maven-style library store
  shared/assets/{indexes,objects}
  shared/mods/                   content-addressed mod store (stage 5)
  shared/java/                   managed JVMs (stage 4)
  instances/<profile>/game/      worlds, mods, configs (per profile)
  cache/                         file index, version manifest
~/.config/kestrel/               state.json, config.json
```

Every profile references the shared store directly; jars, libraries and assets
are verified once and never duplicated.

## CLI

```bash
kestrel                    # open the launcher
kestrel --profile NAME     # open and select/launch a profile
kestrel --join HOST:PORT   # launch and join a server (stage 7)
kestrel bench [--json]     # headless timing harness
kestrel --help
```

## Configuration

Configuration lives in `~/.config/kestrel/config.json` (or `$KESTREL_CONFIG_DIR`).
Cached UI state lives in the same directory; the shared content cache lives in
`~/.local/share/kestrel` (or `$KESTREL_HOME`). Set `KESTREL_HOME` to use a
portable/test environment.

Microsoft OAuth needs an Azure application (client) ID. Set
`KESTREL_AZURE_CLIENT_ID` or `azureClientId` in `config.json`. The app is a
public client using the device-code flow, so it needs no secret and no redirect
URI. Setup details and troubleshooting live in [docs/AUTH.md](docs/AUTH.md).

## Accounts

Click **Sign in**, open the shown `microsoft.com/link` address, and enter the
short code. Kestrel polls Microsoft until you approve, then builds the
Xbox Live → XSTS → Minecraft session. Passwords never touch the launcher and no
embedded browser is used.

- Tokens are stored in the **system keyring** (Secret Service on Linux), never
  in `state.json` or logs.
- Sessions refresh silently on startup; expired sessions ask you to sign in
  again.
- One account is active at a time in stage 3; account management and multiple
  profiles arrive later.
- **Sign out** deletes the keyring entry and the cached account summary.

## Logs and timing

- `~/.local/share/kestrel/logs/kestrel.log` — human-readable log
- `~/.local/share/kestrel/logs/timings.jsonl` — every startup/benchmark sample

## Legal / brand

- Kestrel downloads game files from Mojang's official endpoints only.
- Modrinth requests use an identifiable `User-Agent`.
- No telemetry. Nothing is sent anywhere except the official APIs needed to
  log in, download game content, and query Modrinth.
- Minecraft is a trademark of Mojang Synergies AB.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
