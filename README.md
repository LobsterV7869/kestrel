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
- [ ] Stage 2 — Version manifest, downloads, shared cache, vanilla launch
- [ ] Stage 3 — Microsoft auth (device code) + keyring
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

Benchmarks:

```bash
cargo build --release -p kestrel-app --features custom-protocol
./target/release/kestrel bench
```

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
`KESTREL_AZURE_CLIENT_ID` or `azureClientId` in `config.json`. See
`docs/AUTH.md` (stage 3).

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
