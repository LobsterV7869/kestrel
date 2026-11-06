//! Tauri shell for Kestrel.
//!
//! The shell is intentionally thin: it owns the window, a small amount of
//! in-memory state, and IPC. All real work lives in `kestrel-core` so it can be
//! tested without a display server.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{Emitter, Manager, State};

use kestrel_core::{
    auth::{self, AuthSession},
    bench,
    cache::SharedCache,
    cli::{self, Invocation},
    config::Config,
    content::{
        self, catalog, install as mod_install, modrinth, resolve, InstallItem, InstallPlan,
        InstallProgress, InstanceSummary, ModsCatalog, ModUpdate, ProjectDetail, SearchResult,
    },
    error::Error,
    install,
    instances::InstancePaths,
    java,
    launch::{self, LaunchOptions},
    loaders,
    logging, mods, paths,
    state::{AccountSummary, AppState, Loader, Profile},
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
    pending_login: Mutex<Option<auth::PendingLogin>>,
    /// Serialises session restore/refresh so two callers never rotate the same
    /// refresh token at once.
    auth_lock: tokio::sync::Mutex<()>,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct McVersion {
    id: String,
    kind: String,
    release_time: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateInstanceResult {
    state: AppState,
    warning: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoaderInstallResult {
    state: AppState,
    loader: loaders::InstalledLoader,
}

fn profile_for(ctx: &AppCtx, instance_id: Option<&str>) -> Result<Profile, String> {
    let state = ctx.state.lock().unwrap_or_else(|e| e.into_inner());
    let wanted = instance_id
        .map(|s| s.to_string())
        .or_else(|| state.selected_profile.clone());
    state
        .profiles
        .iter()
        .find(|p| Some(&p.id) == wanted.as_ref())
        .or_else(|| state.profiles.first())
        .cloned()
        .ok_or_else(|| "No instance is selected. Create one first.".to_string())
}

fn instance_for(profile: &Profile) -> Result<InstancePaths, String> {
    let paths = InstancePaths::open(&profile.id).map_err(|e| e.to_string())?;
    paths.ensure().map_err(|e| e.to_string())?;
    Ok(paths)
}

fn instance_summary(profile: &Profile, paths: &InstancePaths) -> InstanceSummary {
    InstanceSummary {
        id: profile.id.clone(),
        name: profile.name.clone(),
        mc_version: profile.mc_version.clone(),
        loader: profile.loader,
        loader_version: profile.loader_version.clone(),
        game_dir: paths.game_dir.to_string_lossy().to_string(),
    }
}

fn require_ready(profile: &Profile) -> Result<(), String> {
    let status = loaders::status(
        profile.loader,
        &profile.mc_version,
        profile.loader_version.as_deref(),
    );
    if status.ready {
        Ok(())
    } else {
        Err(status
            .message
            .unwrap_or_else(|| "The mod loader is not ready for this instance.".into()))
    }
}

fn update_loader_version(ctx: &AppCtx, instance_id: &str, version: &str) -> Result<AppState, String> {
    let mut state = ctx.state.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(profile) = state.profiles.iter_mut().find(|p| p.id == instance_id) {
        profile.loader_version = Some(version.to_string());
    }
    state.touch();
    state.save().map_err(|e| e.to_string())?;
    Ok(state.clone())
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in name.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "instance".to_string()
    } else {
        trimmed.chars().take(40).collect()
    }
}

fn unique_instance_id(name: &str) -> String {
    let base = slugify(name);
    let now = bench::now_epoch_ms();
    for attempt in 0..64u64 {
        let suffix = (now.wrapping_add(attempt * 7919) & 0xffffff) as u32;
        let id = format!("{base}-{suffix:06x}");
        if !paths::instance_dir(&id).exists() {
            return id;
        }
    }
    format!("{base}-{}", std::process::id())
}

#[tauri::command]
async fn list_mc_versions() -> Result<Vec<McVersion>, String> {
    let manifest = install::manifest().await.map_err(|e| e.to_string())?;
    Ok(manifest
        .versions
        .into_iter()
        .filter(|v| v.kind == "release")
        .map(|v| McVersion {
            id: v.id,
            kind: v.kind,
            release_time: v.release_time,
        })
        .collect())
}

#[tauri::command]
async fn create_instance(
    ctx: State<'_, AppCtx>,
    name: String,
    mc_version: String,
    loader: Loader,
    loader_version: Option<String>,
) -> Result<CreateInstanceResult, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the instance a name.".into());
    }
    if name.chars().count() > 48 {
        return Err("Instance names are limited to 48 characters.".into());
    }
    if mc_version.trim().is_empty() {
        return Err("Choose a Minecraft version.".into());
    }

    let id = unique_instance_id(&name);
    let profile = Profile {
        id: id.clone(),
        name,
        mc_version: mc_version.clone(),
        loader,
        loader_version: loader_version.clone(),
        created_at: bench::now_epoch_ms(),
        ..Profile::default()
    };
    let paths = InstancePaths::open(&id).map_err(|e| e.to_string())?;
    paths.ensure().map_err(|e| e.to_string())?;

    {
        let mut state = ctx.state.lock().unwrap_or_else(|e| e.into_inner());
        state.profiles.push(profile.clone());
        state.selected_profile = Some(id.clone());
        state.touch();
        state.save().map_err(|e| e.to_string())?;
    }

    let mut warning = None;
    if loaders::supported(loader) {
        let cache = SharedCache::open().map_err(|e| e.to_string())?;
        match loaders::install(loader, &mc_version, loader_version.as_deref(), &cache).await {
            Ok(installed) => {
                let state = update_loader_version(&ctx, &id, &installed.loader_version)?;
                return Ok(CreateInstanceResult {
                    state,
                    warning: None,
                });
            }
            Err(err) => {
                warning = Some(format!(
                    "The instance was created, but the {} loader could not be installed: {err}",
                    loaders::loader_name(loader)
                ));
            }
        }
    } else if loader != Loader::Vanilla {
        warning = Some(
            "This build installs Fabric and Quilt loaders. Forge/NeoForge support is coming next."
                .into(),
        );
    }

    let state = ctx
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    Ok(CreateInstanceResult { state, warning })
}

#[tauri::command]
async fn install_loader(
    ctx: State<'_, AppCtx>,
    instance_id: String,
) -> Result<LoaderInstallResult, String> {
    let profile = profile_for(&ctx, Some(&instance_id))?;
    if !loaders::supported(profile.loader) {
        return Err(format!(
            "{} loader installation is not supported in this build.",
            loaders::loader_name(profile.loader)
        ));
    }
    let cache = SharedCache::open().map_err(|e| e.to_string())?;
    let installed = loaders::install(
        profile.loader,
        &profile.mc_version,
        profile.loader_version.as_deref(),
        &cache,
    )
    .await
    .map_err(|e| e.to_string())?;
    let state = update_loader_version(&ctx, &profile.id, &installed.loader_version)?;
    Ok(LoaderInstallResult {
        state,
        loader: installed,
    })
}

#[tauri::command]
async fn list_loader_versions(
    loader: Loader,
    mc_version: String,
) -> Result<Vec<loaders::LoaderVersion>, String> {
    loaders::list_versions(loader, &mc_version)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn loader_status(ctx: State<'_, AppCtx>, instance_id: String) -> Result<loaders::LoaderStatus, String> {
    let profile = profile_for(&ctx, Some(&instance_id))?;
    Ok(loaders::status(
        profile.loader,
        &profile.mc_version,
        profile.loader_version.as_deref(),
    ))
}

#[tauri::command]
async fn mods_catalog(
    ctx: State<'_, AppCtx>,
    instance_id: Option<String>,
) -> Result<ModsCatalog, String> {
    let profile = profile_for(&ctx, instance_id.as_deref())?;
    let paths = instance_for(&profile)?;
    let _ = mods::recover(&paths);
    let installed = mods::scan(&paths).map_err(|e| e.to_string())?;
    let status = loaders::status(
        profile.loader,
        &profile.mc_version,
        profile.loader_version.as_deref(),
    );

    let provider = modrinth::ModrinthProvider;
    let features = catalog::resolve_features(
        &provider,
        &profile.mc_version,
        profile.loader,
        &installed,
    )
    .await
    .map_err(|e| e.to_string())?;
    let updates = if status.ready {
        mods::check_updates(&provider, &paths, &profile.mc_version, profile.loader)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(ModsCatalog {
        instance: Some(instance_summary(&profile, &paths)),
        loader_ready: status.ready,
        loader_error: status.message,
        features,
        installed,
        updates,
        warnings: Vec::new(),
    })
}

#[tauri::command]
async fn mods_search(
    ctx: State<'_, AppCtx>,
    instance_id: Option<String>,
    query: String,
    category: String,
    offset: u32,
    limit: u32,
) -> Result<SearchResult, String> {
    let profile = profile_for(&ctx, instance_id.as_deref())?;
    if content::loader_key(profile.loader).is_none() {
        return Err("Select a Fabric or Quilt instance to browse mods.".into());
    }
    modrinth::search(
        &query,
        &profile.mc_version,
        profile.loader,
        &category,
        offset,
        limit,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn mods_project(project_id: String) -> Result<ProjectDetail, String> {
    modrinth::project(&project_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn mods_versions(
    ctx: State<'_, AppCtx>,
    instance_id: Option<String>,
    project_id: String,
) -> Result<Vec<content::ModVersion>, String> {
    let profile = profile_for(&ctx, instance_id.as_deref())?;
    modrinth::versions(&project_id, &profile.mc_version, profile.loader)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn mods_plan(
    ctx: State<'_, AppCtx>,
    instance_id: String,
    items: Vec<InstallItem>,
) -> Result<InstallPlan, String> {
    let profile = profile_for(&ctx, Some(&instance_id))?;
    require_ready(&profile)?;
    let paths = instance_for(&profile)?;
    let installed = mods::scan(&paths).map_err(|e| e.to_string())?;
    let provider = modrinth::ModrinthProvider;
    resolve::plan(
        &provider,
        &profile.id,
        &profile.mc_version,
        profile.loader,
        &items,
        &installed,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn mods_install(
    app: tauri::AppHandle,
    ctx: State<'_, AppCtx>,
    instance_id: String,
    items: Vec<InstallItem>,
) -> Result<content::InstallOutcome, String> {
    let profile = profile_for(&ctx, Some(&instance_id))?;
    require_ready(&profile)?;
    let paths = instance_for(&profile)?;
    let installed = mods::scan(&paths).map_err(|e| e.to_string())?;
    let provider = modrinth::ModrinthProvider;
    let plan = resolve::plan(
        &provider,
        &profile.id,
        &profile.mc_version,
        profile.loader,
        &items,
        &installed,
    )
    .await
    .map_err(|e| e.to_string())?;

    let progress_app = app.clone();
    let sink: mod_install::ProgressFn =
        std::sync::Arc::new(move |progress: InstallProgress| {
            let _ = progress_app.emit("kestrel://mods-progress", progress);
        });
    let outcome = mod_install::execute(&paths, &plan, sink)
        .await
        .map_err(|e| e.to_string())?;
    let _ = app.emit("kestrel://mods-changed", &profile.id);
    Ok(outcome)
}

#[tauri::command]
async fn mods_check_updates(
    ctx: State<'_, AppCtx>,
    instance_id: String,
) -> Result<Vec<ModUpdate>, String> {
    let profile = profile_for(&ctx, Some(&instance_id))?;
    let paths = instance_for(&profile)?;
    let provider = modrinth::ModrinthProvider;
    mods::check_updates(&provider, &paths, &profile.mc_version, profile.loader)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn mods_update(
    app: tauri::AppHandle,
    ctx: State<'_, AppCtx>,
    instance_id: String,
    project_ids: Vec<String>,
) -> Result<content::InstallOutcome, String> {
    let profile = profile_for(&ctx, Some(&instance_id))?;
    require_ready(&profile)?;
    let paths = instance_for(&profile)?;
    let installed = mods::scan(&paths).map_err(|e| e.to_string())?;
    let provider = modrinth::ModrinthProvider;
    let plan = resolve::update_plan(
        &provider,
        &profile.id,
        &profile.mc_version,
        profile.loader,
        &project_ids,
        &installed,
    )
    .await
    .map_err(|e| e.to_string())?;

    if plan.items.is_empty() {
        return Ok(content::InstallOutcome {
            transaction_id: format!("noop-{}", bench::now_epoch_ms()),
            instance_id: profile.id,
            installed: Vec::new(),
            warnings: plan.warnings,
        });
    }

    let progress_app = app.clone();
    let sink: mod_install::ProgressFn =
        std::sync::Arc::new(move |progress: InstallProgress| {
            let _ = progress_app.emit("kestrel://mods-progress", progress);
        });
    let outcome = mod_install::execute(&paths, &plan, sink)
        .await
        .map_err(|e| e.to_string())?;
    let _ = app.emit("kestrel://mods-changed", &profile.id);
    Ok(outcome)
}

#[tauri::command]
fn mods_installed(ctx: State<'_, AppCtx>, instance_id: String) -> Result<Vec<mods::InstalledMod>, String> {
    let profile = profile_for(&ctx, Some(&instance_id))?;
    let paths = instance_for(&profile)?;
    let _ = mods::recover(&paths);
    mods::scan(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
fn mods_set_enabled(
    ctx: State<'_, AppCtx>,
    instance_id: String,
    file_name: String,
    enabled: bool,
) -> Result<Vec<mods::InstalledMod>, String> {
    let profile = profile_for(&ctx, Some(&instance_id))?;
    let paths = instance_for(&profile)?;
    mods::set_enabled(&paths, &file_name, enabled).map_err(|e| e.to_string())?;
    mods::scan(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
fn mods_remove(
    ctx: State<'_, AppCtx>,
    instance_id: String,
    file_name: String,
) -> Result<Vec<mods::InstalledMod>, String> {
    let profile = profile_for(&ctx, Some(&instance_id))?;
    let paths = instance_for(&profile)?;
    mods::remove(&paths, &file_name).map_err(|e| e.to_string())?;
    mods::scan(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
fn mods_import_local(
    ctx: State<'_, AppCtx>,
    instance_id: String,
    path: String,
) -> Result<Vec<mods::InstalledMod>, String> {
    let profile = profile_for(&ctx, Some(&instance_id))?;
    let paths = instance_for(&profile)?;
    mods::import_local(&paths, std::path::Path::new(&path)).map_err(|e| e.to_string())?;
    mods::scan(&paths).map_err(|e| e.to_string())
}

/// Import a jar that the UI read from a file input. Drag-and-drop uses
/// `mods_import_local` instead, which avoids copying the bytes through IPC.
#[tauri::command]
fn mods_import_bytes(
    ctx: State<'_, AppCtx>,
    instance_id: String,
    file_name: String,
    bytes: Vec<u8>,
) -> Result<Vec<mods::InstalledMod>, String> {
    const MAX: usize = 128 * 1024 * 1024;
    if bytes.is_empty() {
        return Err("The selected file is empty.".into());
    }
    if bytes.len() > MAX {
        return Err("The selected jar is larger than 128 MB.".into());
    }
    let profile = profile_for(&ctx, Some(&instance_id))?;
    let paths = instance_for(&profile)?;
    let name = file_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("mod.jar")
        .to_string();
    if !name.to_lowercase().ends_with(".jar") {
        return Err("Only .jar mods can be imported.".into());
    }
    let dir = paths.kestrel_dir().join("imports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{}-{name}", bench::now_epoch_ms()));
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    let result = mods::import_local(&paths, &tmp);
    let _ = std::fs::remove_file(&tmp);
    result.map_err(|e| e.to_string())?;
    mods::scan(&paths).map_err(|e| e.to_string())
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
            println!(
                "kestrel {} ({})",
                env!("CARGO_PKG_VERSION"),
                kestrel_core::APP_NAME
            );
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
        pending_login: Mutex::new(None),
        auth_lock: tokio::sync::Mutex::new(()),
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
            get_config,
            auth_start,
            auth_poll,
            auth_restore,
            auth_logout,
            open_external,
            list_mc_versions,
            create_instance,
            install_loader,
            list_loader_versions,
            loader_status,
            mods_catalog,
            mods_search,
            mods_project,
            mods_versions,
            mods_plan,
            mods_install,
            mods_check_updates,
            mods_update,
            mods_installed,
            mods_set_enabled,
            mods_remove,
            mods_import_local,
            mods_import_bytes
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

            // Silent session refresh after the first frame. Emitting an event
            // keeps the UI in sync even if it missed the initial state.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let ctx = handle.state::<AppCtx>();
                if ctx.config.azure_client_id.trim().is_empty() {
                    return;
                }
                let started = std::time::Instant::now();
                let result = restore_account(&ctx).await;
                tracing::info!(
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "silent account restore finished"
                );
                match result {
                    Ok(account) => {
                        let _ = handle.emit("kestrel://account", account);
                    }
                    Err(err) => {
                        let _ = handle.emit("kestrel://auth-error", err);
                    }
                }
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
        println!(
            "heap {memory_mb} MiB, game dir {}",
            instance.game_dir.display()
        );
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthPoll {
    status: &'static str,
    account: Option<AccountSummary>,
}

impl AuthPoll {
    fn new(status: &'static str, account: Option<AccountSummary>) -> Self {
        Self { status, account }
    }
}

fn pending_guard(ctx: &AppCtx) -> std::sync::MutexGuard<'_, Option<auth::PendingLogin>> {
    ctx.pending_login.lock().unwrap_or_else(|e| e.into_inner())
}

fn persist_account(ctx: &AppCtx, session: &AuthSession) -> Result<AccountSummary, String> {
    let summary = session.summary();
    let mut state = ctx.state.lock().unwrap_or_else(|e| e.into_inner());
    state.accounts = vec![summary.clone()];
    state.touch();
    state.save().map_err(|e| e.to_string())?;
    Ok(summary)
}

fn clear_account(ctx: &AppCtx) -> Result<(), String> {
    let mut state = ctx.state.lock().unwrap_or_else(|e| e.into_inner());
    if state.accounts.is_empty() {
        return Ok(());
    }
    state.accounts.clear();
    state.touch();
    state.save().map_err(|e| e.to_string())
}

async fn restore_account(ctx: &AppCtx) -> Result<Option<AccountSummary>, String> {
    let client_id = ctx.config.azure_client_id.trim().to_string();
    if client_id.is_empty() {
        clear_account(ctx)?;
        return Ok(None);
    }
    let _guard = ctx.auth_lock.lock().await;
    match auth::restore(&client_id).await {
        Ok(Some(session)) => persist_account(ctx, &session).map(Some),
        Ok(None) => {
            clear_account(ctx)?;
            Ok(None)
        }
        Err(Error::AuthExpired) => {
            clear_account(ctx)?;
            Err("Microsoft session expired; sign in again.".to_string())
        }
        Err(Error::NoOwnership) => {
            clear_account(ctx)?;
            Err("This Microsoft account does not own Minecraft: Java Edition.".to_string())
        }
        Err(err) => Err(err.to_string()),
    }
}

#[tauri::command]
async fn auth_start(ctx: State<'_, AppCtx>) -> Result<auth::DeviceCodeInfo, String> {
    let client_id = ctx.config.azure_client_id.trim().to_string();
    tracing::info!("starting Microsoft device-code flow");
    let info = auth::start_device_code(&client_id).await.map_err(|e| {
        tracing::warn!(error = %e, "device-code request failed");
        e.to_string()
    })?;
    *pending_guard(&ctx) = Some(auth::PendingLogin::new(info.clone()));
    Ok(info)
}

#[tauri::command]
async fn auth_poll(app: tauri::AppHandle, ctx: State<'_, AppCtx>) -> Result<AuthPoll, String> {
    let client_id = ctx.config.azure_client_id.trim().to_string();
    let pending = pending_guard(&ctx).clone();
    let Some(pending) = pending else {
        return Err("no sign-in is in progress".to_string());
    };
    if pending.is_expired() {
        *pending_guard(&ctx) = None;
        return Ok(AuthPoll::new("expired", None));
    }
    let outcome = auth::poll_device_code(&client_id, &pending.info.device_code)
        .await
        .map_err(|e| e.to_string())?;
    tracing::debug!(?outcome, "device-code poll");
    match outcome {
        auth::PollOutcome::Pending => Ok(AuthPoll::new("pending", None)),
        auth::PollOutcome::SlowDown => Ok(AuthPoll::new("slowDown", None)),
        auth::PollOutcome::Expired => {
            *pending_guard(&ctx) = None;
            Ok(AuthPoll::new("expired", None))
        }
        auth::PollOutcome::Declined => {
            *pending_guard(&ctx) = None;
            Ok(AuthPoll::new("declined", None))
        }
        auth::PollOutcome::Complete(token) => {
            // The device code is single-use; never keep it around after it
            // produced a token, even if the Xbox/Minecraft chain fails.
            *pending_guard(&ctx) = None;
            // Serialise with startup restore so an in-flight refresh cannot
            // overwrite the tokens this login just stored.
            let _guard = ctx.auth_lock.lock().await;
            let (session, _tokens) = auth::complete_login(&client_id, token)
                .await
                .map_err(|e| e.to_string())?;
            let account = persist_account(&ctx, &session)?;
            let _ = app.emit("kestrel://account", Some(account.clone()));
            Ok(AuthPoll::new("success", Some(account)))
        }
    }
}

#[tauri::command]
async fn auth_restore(
    app: tauri::AppHandle,
    ctx: State<'_, AppCtx>,
) -> Result<Option<AccountSummary>, String> {
    let result = restore_account(&ctx).await;
    match &result {
        Ok(account) => {
            let _ = app.emit("kestrel://account", account.clone());
        }
        Err(err) => {
            let _ = app.emit("kestrel://auth-error", err.clone());
        }
    }
    result
}

#[tauri::command]
async fn auth_logout(app: tauri::AppHandle, ctx: State<'_, AppCtx>) -> Result<(), String> {
    let _guard = ctx.auth_lock.lock().await;
    auth::delete_tokens().map_err(|e| e.to_string())?;
    *pending_guard(&ctx) = None;
    clear_account(&ctx)?;
    let none: Option<AccountSummary> = None;
    let _ = app.emit("kestrel://account", none);
    Ok(())
}

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("refusing to open a non-HTTP URL".to_string());
    }
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("could not open the browser: {e}"))?;
    Ok(())
}
