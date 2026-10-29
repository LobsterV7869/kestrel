# Performance log

Every claim here is measured on the machine below with a **release** build.
Numbers are copied from `~/.local/share/kestrel/logs/timings.jsonl`, produced by
the in-app timing log and `kestrel bench`. If an optimization does not move a
number, it gets removed.

## Machine

| | |
|---|---|
| CPU | Intel Core i5-8365U (4C/8T, 1.6 GHz base, 4.1 GHz boost) |
| RAM | 7.1 GiB (swap: 14 GiB, zram enabled) |
| GPU | Intel UHD Graphics 620 (Whiskey Lake), Mesa 26.2.1 |
| OS | Omarchy 4.0.2, kernel 7.1.9-arch1-2 |
| Session | Wayland / Hyprland 0.56.2 |
| WebView | WebKitGTK 2.52.6 |
| Toolchain | rustc 1.98.0, Node 26.7.0 |

Note: measurements were taken with the desktop busy (load average ~6, other
applications running), which is the realistic case for a launcher. Absolute
numbers will be lower on an idle machine.

## Definitions

- **backend_ready** — Rust has created directories and loaded cached state +
  config. This must never touch the network.
- **window_ready** — the Tauri setup hook ran; the GTK window exists and is
  shown with the configured dark background.
- **js_module_start** — the JS bundle has been parsed and is executing. Measured
  from process start.
- **js_mounted** — Solid has mounted and issued its first IPC call.
- **first_paint** — the UI has rendered from cached state and two animation
  frames have elapsed. Measured with `Date.now()` against the backend's process
  start timestamp.

## Baseline — Stage 1 (2026-10-29)

Startup, release build, three consecutive runs (cold-ish; the webview process is
killed between runs):

| Run | backend_ready | window_ready | bundle start | JS mounted | first paint |
|----:|--------------:|-------------:|-------------:|-----------:|------------:|
| 1 | 4 ms | 503 ms | 71 ms | 90 ms | 1501 ms |
| 2 | 4 ms | 526 ms | 46 ms | 64 ms | 1303 ms |
| 3 | 6 ms | 528 ms | 60 ms | 79 ms | 1854 ms |

Interpretation:

- The launcher's own work is **~5 ms** to cached state, and the JS bundle mounts
  in **~60–90 ms**. Our code is not the bottleneck.
- Nearly all of the remaining time is **WebKitGTK/webview startup** (GTK window +
  WebKitWebProcess + first composited frame). Disabling DMABUF rendering or the
  compositor (`WEBKIT_DISABLE_DMABUF_RENDERER=1`,
  `WEBKIT_DISABLE_COMPOSITING_MODE=1`) changed first paint by less than noise
  (1252–1317 ms), so neither toggle is kept.
- The window is on screen with the correct background at ~500 ms; content arrives
  at ~1.3 s. The <300 ms target is **not met yet** and is the main subject of
  stage 6.

`kestrel bench` (release, single run):

| Phase | Time | Notes |
|---|---:|---|
| cache_load | 0.08 ms | cached state read |
| state_save | 0.96 ms | atomic write, 120 B |
| sha1_32mib | 256.99 ms | 125 MiB/s (asset verification path) |
| json_parse_2k | 10.93 ms | 495 KiB synthetic manifest |
| net_manifest | 625.90 ms | 271 KiB from piston-meta (network-bound) |

Debug builds of the SHA-1 path are ~30× slower (7 MiB/s); always benchmark in
release. Never ship debug timing numbers.

## Stage 2 — vanilla install and launch (2026-10-29)

| Operation | Time | Notes |
|---|---:|---|
| cold install of 1.20.4 | 123.5 s | 52 libraries + 3811 assets, ~600 MB, concurrency 24–32 |
| warm `kestrel install 1.20.4` | 0.42–0.80 s | 3811 stat checks against the file index, no network |
| click to title screen | ~35 s | first run, empty page cache, no AppCDS; debug launcher |

The warm install proves the index works: nothing is re-hashed or re-downloaded.
The remaining ~0.4 s is 3863 `stat` calls; stage 6 will trust the index for a
whole tree and only revalidate when the asset index changes.

Launch timing includes JVM start, 198 datafixer optimisations (~0.7 s), LWJGL
init and texture atlas creation. Stage 6 attacks this with page-cache pre-warm,
AppCDS and JVM flags; the target is under 10 s to the title screen on warm disk.

Observed harmless noise on 1.20.4: the narrator fails to load `libflite.so` and
the dev token gets a 401 when authlib fetches user properties. Neither stops the
game from reaching the main menu.

## Optimizations tried

| Change | Effect | Kept? |
|---|---|---|
| Cached state read before any network | backend ready 4–6 ms | yes |
| Inline critical CSS skeleton | paints before JS bundle | yes |
| Tiny frontend (17.6 kB JS / 4.2 kB CSS) | bundle executes in <10 ms after parse | yes |
| `WEBKIT_DISABLE_DMABUF_RENDERER=1` | no measurable gain | no |
| `WEBKIT_DISABLE_COMPOSITING_MODE=1` | no measurable gain | no |
| `custom-protocol` asset embedding | needed for release to run without a dev server | yes |

## Next measurements (stage 6)

- Page-cache pre-warm of jars/libraries/assets.
- AppCDS archive creation and reuse (per profile).
- JVM presets: generational ZGC vs G1/Aikar, `-Xms=-Xmx`, no AlwaysPreTouch.
- File-index verification skip (size+mtime, hash only on mismatch).
- Click-to-game-window / click-to-main-menu / click-to-in-server.
