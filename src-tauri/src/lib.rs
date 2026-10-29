//! Tauri shell for Kestrel.
//!
//! The shell is intentionally thin: it owns the window, a small amount of
//! in-memory state, and IPC. All real work lives in `kestrel-core` so it can be
//! tested without a display server.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{Emitter, Manager, State};

use kestrel_core::{
    auth::AuthSession,
    bench,
    cache::SharedCache,
    cli::{self, Invocation},
    config::Config,
    error::Error,
    install,
    instances::InstancePaths,
    java,
    launch::{self, LaunchOptions},
    logging, paths,
    state::AppState,
};

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRequest {
    pub profile: Option<String>,
    pub join: Option<String>,
}

struct AppCtx {
    state: Mutex<AppState>,
    config: Config,
    launch: LaunchRequest,
    backend_ready_ms: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    state: AppState,
    process_start_epoch_ms: u64,
    now_epoch_ms: u64,
    backend_ready_ms: f64,
    app_name: &'static str,
    app_version: &'static str,
    legal: &'static str,
    azure_configured: bool,
    launch: LaunchRequest,
}

#[tauri::command]
fn bootstrap(ctx: State<'_, AppCtx>) -> Bootstrap {
    let state = ctx.state.lock().map(|s| s.clone()).unwrap_or_default();
    Bootstrap {
        state,
        process_start_epoch_ms: bench::process_start_epoch_ms(),
        now_epoch_ms: bench::now_epoch_ms(),
        backend_ready_ms: ctx.backend_ready_ms,
        app_name: kestrel_core::APP_NAME,
        app_version: env!("CARGO_PKG_VERSION"),
        legal: kestrel_core::LEGAL_NOTICE,
        azure_configured: !ctx.config.azure_client_id.trim().is_empty(),
        launch: ctx.launch.clone(),
    }
}

#[tauri::command]
fn save_state(ctx: State<'_, AppCtx>, state: AppState) -> Result<(), String> {
    state.save().map_err(|e| e.to_string())?;
    if let Ok(mut guard) = ctx.state.lock() {
        *guard = state;
    }
    Ok(())
}

#[tauri::command]
fn log_timing(name: String, elapsed_ms: f64, meta: Option<String>) {
    logging::record_timing(&paths::logs_dir(), &name, elapsed_ms, meta.as_deref());
    tracing::debug!(target: "timing", "{name}: {elapsed_ms:.2} ms");
}

#[tauri::command]
fn get_config(ctx: State<'_, AppCtx>) -> Config {
    ctx.config.clone()
}

/// Entry point shared by the binary and (later) mobile targets.
pub fn entry() {
    let _ = paths::ensure_dirs();
    logging::init(&paths::logs_dir());

    match cli::parse_args(std::env::args().skip(1)) {
        Ok(Invocation::Bench { json }) => {
            if let Err(err) = bench::run(json) {
                eprintln!("bench failed: {err}");
                std::process::exit(1);
            }
        }
        Ok(Invocation::Version) => {
            println!("kestrel {} ({})", env!("CARGO_PKG_VERSION"), kestrel_core::APP_NAME);
        }
        Ok(Invocation::Help) => print!("{}", cli::HELP),
        Ok(Invocation::Install { version }) => run_install(version),
        Ok(Invocation::DevLaunch { version }) => run_dev_launch(version),
        Ok(Invocation::Gui { profile, join }) => run_gui(profile, join),
        Err(err) => {
            eprintln!("error: {err}\n\n{}", cli::HELP);
            std::process::exit(2);
        }
    }
}

fn run_gui(profile: Option<String>, join: Option<String>) {
    let state = AppState::load_or_default();
    let config = Config::load();

    tracing::info!(
        target: "startup",
        profiles = state.profiles.len(),
        accounts = state.accounts.len(),
        "cached state loaded"
    );

    let ctx = AppCtx {
        state: Mutex::new(state),
        config,
        launch: LaunchRequest { profile, join },
        backend_ready_ms: bench::since_start_ms(),
    };

    let mut timeline = bench::Timeline::new();
    timeline.mark("backend_ready").record();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .manage(ctx)
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            save_state,
            log_timing,
            get_config
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
            }
            logging::record_timing(
                &paths::logs_dir(),
                "window_ready",
                bench::since_start_ms(),
                Some("ui"),
            );
            // Background work is scheduled here and must never block the
            // first paint. Stage 2 wires manifest/mod updates into this task.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let _ = handle.emit("kestrel://background-idle", ());
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Kestrel");
}

fn runtime() -> Result<tokio::runtime::Runtime, Error> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::msg(format!("tokio runtime: {e}")))
}

fn run_install(version: String) {
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    };
    let result = rt.block_on(async {
        let cache = SharedCache::open()?;
        let started = std::time::Instant::now();
        let installed = install::install_version(&version, &cache).await?;
        Ok::<_, Error>((installed, started.elapsed()))
    });
    match result {
        Ok((installed, elapsed)) => {
            println!(
                "installed {} in {:.1}s",
                installed.id,
                elapsed.as_secs_f64()
            );
            println!("  client jar   {}", installed.jar.display());
            println!("  libraries    {}", installed.classpath.len());
            println!("  natives      {}", installed.natives_dir.display());
            println!("  assets index {}", installed.assets_index);
            println!("  java         >= {}", installed.java_major());
        }
        Err(err) => {
            eprintln!("install failed: {err}");
            std::process::exit(1);
        }
    }
}

fn run_dev_launch(version: String) {
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    };
    let result: Result<(), Error> = rt.block_on(async {
        let auth = AuthSession::dev()?;
        let cache = SharedCache::open()?;
        println!("installing {version} ...");
        let installed = install::install_version(&version, &cache).await?;
        let java = java::find_for(installed.java_major()).ok_or_else(|| {
            Error::msg(format!(
                "no Java {} runtime found; install one or set PATH",
                installed.java_major()
            ))
        })?;
        let instance = InstancePaths::new("_dev");
        instance.ensure()?;
        let memory_mb = launch::default_memory_mb();
        let options = LaunchOptions {
            version: installed.json.clone(),
            version_name: installed.id.clone(),
            client_jar: installed.jar.clone(),
            classpath: installed.classpath.clone(),
            natives_dir: installed.natives_dir.clone(),
            assets_dir: installed.assets_dir.clone(),
            assets_index: installed.assets_index.clone(),
            game_dir: instance.game_dir.clone(),
            auth,
            java: java.clone(),
            memory_mb,
            extra_jvm: Vec::new(),
            extra_game: Vec::new(),
            quick_play: None,
            width: None,
            height: None,
        };
        let spec = launch::build(&options)?;
        println!(
            "using Java {} ({}) at {}",
            java.version,
            java.major,
            java.path.display()
        );
        println!("heap {memory_mb} MiB, game dir {}", instance.game_dir.display());
        println!("classpath entries: {}", spec.classpath_len());
        let mut child = launch::spawn(&spec).await?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::msg("missing stdout pipe"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::msg("missing stderr pipe"))?;
        let out_task = tokio::spawn(stream_output(stdout, false));
        let err_task = tokio::spawn(stream_output(stderr, true));
        let status = child.wait().await?;
        let _ = out_task.await;
        let _ = err_task.await;
        if status.success() {
            println!("Minecraft exited cleanly");
            Ok(())
        } else {
            Err(Error::msg(format!("Minecraft exited with {status}")))
        }
    });
    if let Err(err) = result {
        eprintln!("launch failed: {err}");
        std::process::exit(1);
    }
}

async fn stream_output<R>(reader: R, stderr: bool)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if stderr {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    }
}
