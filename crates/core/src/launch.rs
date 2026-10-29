//! Command-line construction and process supervision for the Minecraft client.
//!
//! The builder is pure: it takes a resolved [`VersionJson`] and produces the
//! exact program/args/cwd. That makes it unit-testable and keeps process
//! spawning separate from the launch policy.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::auth::AuthSession;
use crate::error::{Error, Result};
use crate::java::JavaInstall;
use crate::versions::{expand_arguments, expand_legacy, RuleContext, VersionJson};

#[derive(Debug, Clone)]
pub enum QuickPlay {
    Singleplayer(String),
    Multiplayer(String),
    Realms(String),
}

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub version: VersionJson,
    /// Version folder name, also passed as `--version`.
    pub version_name: String,
    pub client_jar: PathBuf,
    pub classpath: Vec<PathBuf>,
    pub natives_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub assets_index: String,
    pub game_dir: PathBuf,
    pub auth: AuthSession,
    pub java: JavaInstall,
    pub memory_mb: u32,
    pub extra_jvm: Vec<String>,
    pub extra_game: Vec<String>,
    pub quick_play: Option<QuickPlay>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

impl CommandSpec {
    pub fn classpath_len(&self) -> usize {
        self.args
            .iter()
            .position(|arg| arg == "-cp" || arg == "-classpath")
            .and_then(|index| self.args.get(index + 1))
            .map(|cp| cp.split(':').count())
            .unwrap_or(0)
    }
}

pub fn build(opts: &LaunchOptions) -> Result<CommandSpec> {
    let mut classpath = Vec::with_capacity(opts.classpath.len() + 1);
    classpath.push(opts.client_jar.clone());
    classpath.extend(opts.classpath.iter().cloned());
    let separator = if cfg!(windows) { ";" } else { ":" };
    let classpath = classpath
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(separator);

    let mut features: HashMap<String, bool> = HashMap::new();
    let mut quick_values: HashMap<String, String> = HashMap::new();
    match &opts.quick_play {
        Some(QuickPlay::Singleplayer(world)) => {
            features.insert("is_quick_play_singleplayer".into(), true);
            quick_values.insert("quickPlaySingleplayer".into(), world.clone());
        }
        Some(QuickPlay::Multiplayer(address)) => {
            features.insert("is_quick_play_multiplayer".into(), true);
            quick_values.insert("quickPlayMultiplayer".into(), address.clone());
        }
        Some(QuickPlay::Realms(id)) => {
            features.insert("is_quick_play_realms".into(), true);
            quick_values.insert("quickPlayRealms".into(), id.clone());
        }
        None => {}
    }
    if opts.width.is_some() && opts.height.is_some() {
        features.insert("has_custom_resolution".into(), true);
    }
    let ctx = RuleContext::current().with_features(features);

    let mut values: HashMap<String, String> = HashMap::from([
        ("auth_player_name".into(), opts.auth.username.clone()),
        ("auth_uuid".into(), opts.auth.uuid_undashed()),
        ("auth_access_token".into(), opts.auth.access_token.clone()),
        ("auth_session".into(), format!("token:{}", opts.auth.access_token)),
        ("clientid".into(), opts.auth.client_id.clone()),
        ("auth_xuid".into(), opts.auth.xuid.clone()),
        ("user_type".into(), opts.auth.user_type.clone()),
        ("user_properties".into(), "{}".into()),
        ("version_name".into(), opts.version_name.clone()),
        (
            "version_type".into(),
            opts.version.kind.clone().unwrap_or_else(|| "release".into()),
        ),
        (
            "game_directory".into(),
            opts.game_dir.to_string_lossy().into_owned(),
        ),
        (
            "assets_root".into(),
            opts.assets_dir.to_string_lossy().into_owned(),
        ),
        (
            "game_assets".into(),
            opts.assets_dir.to_string_lossy().into_owned(),
        ),
        ("assets_index_name".into(), opts.assets_index.clone()),
        (
            "natives_directory".into(),
            opts.natives_dir.to_string_lossy().into_owned(),
        ),
        ("launcher_name".into(), "kestrel".into()),
        ("launcher_version".into(), crate::http::VERSION.into()),
        ("classpath".into(), classpath),
    ]);
    values.extend(quick_values);
    if let (Some(width), Some(height)) = (opts.width, opts.height) {
        values.insert("resolution_width".into(), width.to_string());
        values.insert("resolution_height".into(), height.to_string());
    }

    let native_flag = format!(
        "-Djava.library.path={}",
        opts.natives_dir.to_string_lossy()
    );
    let mut args = vec![
        format!("-Xms{}M", opts.memory_mb),
        format!("-Xmx{}M", opts.memory_mb),
    ];

    let jvm = opts
        .version
        .arguments
        .as_ref()
        .map(|arguments| expand_arguments(&arguments.jvm, &ctx, &values))
        .unwrap_or_default();
    if jvm.is_empty() {
        args.push(native_flag);
        args.push("-cp".into());
        args.push(values["classpath"].clone());
    } else {
        args.extend(jvm);
    }
    if !args.iter().any(|arg| arg.starts_with("-Dminecraft.launcher.brand=")) {
        args.push("-Dminecraft.launcher.brand=kestrel".into());
        args.push(format!(
            "-Dminecraft.launcher.version={}",
            crate::http::VERSION
        ));
    }
    args.extend(opts.extra_jvm.iter().cloned());

    if opts.version.main_class.trim().is_empty() {
        return Err(Error::msg("version JSON has no main class"));
    }
    args.push(opts.version.main_class.clone());

    let mut game = if let Some(arguments) = &opts.version.arguments {
        if arguments.game.is_empty() {
            Vec::new()
        } else {
            expand_arguments(&arguments.game, &ctx, &values)
        }
    } else {
        Vec::new()
    };
    if game.is_empty() {
        if let Some(legacy) = &opts.version.minecraft_arguments {
            game = expand_legacy(legacy, &values);
        }
    }
    if game.is_empty() {
        return Err(Error::msg(
            "version has no game arguments (corrupt version JSON)",
        ));
    }

    // Versions older than 1.20 do not know --quickPlayMultiplayer; fall back to
    // the legacy --server/--port pair, which they do understand.
    if let Some(QuickPlay::Multiplayer(address)) = &opts.quick_play {
        if !game.iter().any(|arg| arg == "--quickPlayMultiplayer") {
            let (host, port) = split_address(address);
            game.push("--server".into());
            game.push(host);
            game.push("--port".into());
            game.push(port.to_string());
        }
    }
    game.extend(opts.extra_game.iter().cloned());
    args.extend(game);

    Ok(CommandSpec {
        program: opts.java.path.clone(),
        args,
        cwd: opts.game_dir.clone(),
        env: Vec::new(),
    })
}

fn split_address(address: &str) -> (String, u16) {
    match address.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() => {
            (host.to_string(), port.parse().unwrap_or(25565))
        }
        _ => (address.to_string(), 25565),
    }
}

pub async fn spawn(spec: &CommandSpec) -> Result<tokio::process::Child> {
    let mut command = tokio::process::Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .envs(spec.env.iter().cloned())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command.spawn().map_err(Error::from)
}

/// Rough default heap: half of RAM, rounded to 128 MiB, kept between 1 and 8 GiB.
pub fn default_memory_mb() -> u32 {
    let total = total_ram_mb().unwrap_or(4096);
    let half = total / 2;
    let clamped = half.clamp(1024, 8192);
    ((clamped / 128) * 128) as u32
}

fn total_ram_mb() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CrashKind {
    OutOfMemory,
    WrongJava,
    MissingMainClass,
    Graphics,
    Jvm,
    Mod,
    Exited,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashReport {
    pub kind: CrashKind,
    pub summary: String,
    pub hints: Vec<String>,
}

/// Turn an exit status plus the tail of the game log into a user-facing report.
/// Returns `None` for a clean exit.
pub fn diagnose(exit_code: Option<i32>, log: &str) -> Option<CrashReport> {
    if exit_code == Some(0) {
        return None;
    }

    let check = |needle: &str| log.contains(needle);
    let (kind, summary) = if check("java.lang.OutOfMemoryError") {
        (
            CrashKind::OutOfMemory,
            "The game ran out of memory.".to_string(),
        )
    } else if check("UnsupportedClassVersionError") {
        (
            CrashKind::WrongJava,
            "The selected Java version is too old for this Minecraft version.".to_string(),
        )
    } else if check("Could not find or load main class") || check("ClassNotFoundException") {
        (
            CrashKind::MissingMainClass,
            "Minecraft's main class could not be loaded.".to_string(),
        )
    } else if check("Failed to create window")
        || check("GLFW error")
        || check("No OpenGL context")
        || check("org.lwjgl.LWJGLException")
    {
        (
            CrashKind::Graphics,
            "Minecraft could not create an OpenGL window.".to_string(),
        )
    } else if check("A fatal error has been detected by the Java Runtime Environment")
        || check("SIGSEGV")
        || check("EXCEPTION_ACCESS_VIOLATION")
    {
        (
            CrashKind::Jvm,
            "The Java runtime crashed.".to_string(),
        )
    } else if let Some(description) = crash_description(log) {
        (CrashKind::Mod, description)
    } else {
        (
            CrashKind::Exited,
            format!(
                "Minecraft exited with code {}.",
                exit_code.map(|c| c.to_string()).unwrap_or_else(|| "unknown".into())
            ),
        )
    };

    let hints = match kind {
        CrashKind::OutOfMemory => vec![
            "Increase this profile's memory allocation.".into(),
            "On 32-bit Java the heap cannot exceed roughly 1.5 GB.".into(),
            "Look for mods that keep chunks or entities loaded.".into(),
        ],
        CrashKind::WrongJava => vec![
            "Change the profile's Java runtime to the version the game requires.".into(),
            "Install a managed runtime from the Java tab (stage 4).".into(),
        ],
        CrashKind::MissingMainClass => vec![
            "Reinstall the profile so its jar and loader libraries are restored.".into(),
        ],
        CrashKind::Graphics => vec![
            "Update your Mesa or GPU driver.".into(),
            "Try a different renderer such as Sodium.".into(),
            "Make sure the game is running inside a working graphical session.".into(),
        ],
        CrashKind::Jvm => vec![
            "Open the newest hs_err_pid*.log in the instance folder for details.".into(),
            "Remove recent JVM arguments or mods one at a time.".into(),
        ],
        CrashKind::Mod => vec![
            "Read the Minecraft crash report above the stack trace.".into(),
            "Remove the most recently added mod and try again.".into(),
        ],
        CrashKind::Exited => vec![
            "Search the game log for the first exception, not the last.".into(),
        ],
    };

    Some(CrashReport {
        kind,
        summary,
        hints,
    })
}

fn crash_description(log: &str) -> Option<String> {
    if !log.contains("---- Minecraft Crash Report ----") {
        return None;
    }
    for line in log.lines() {
        if let Some(rest) = line.strip_prefix("Description:") {
            let rest = rest.trim();
            if !rest.is_empty() {
                return Some(format!("Minecraft crashed: {rest}"));
            }
        }
    }
    Some("Minecraft crashed with an unhandled exception.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::versions::{Artifact, Arguments, VersionDownloads};

    fn version(jvm: Vec<crate::versions::Argument>, game: Vec<crate::versions::Argument>) -> VersionJson {
        VersionJson {
            id: "1.20.4".into(),
            kind: Some("release".into()),
            main_class: "net.minecraft.client.main.Main".into(),
            asset_index: None,
            assets: None,
            downloads: VersionDownloads::default(),
            libraries: Vec::new(),
            java_version: None,
            arguments: Some(Arguments { game, jvm }),
            minecraft_arguments: None,
            logging: None,
            compliance_level: None,
            release_time: None,
            time: None,
            minimum_launcher_version: None,
        }
    }

    fn opts(version: VersionJson) -> LaunchOptions {
        LaunchOptions {
            version,
            version_name: "1.20.4".into(),
            client_jar: PathBuf::from("/cache/1.20.4.jar"),
            classpath: vec![PathBuf::from("/cache/lib.jar")],
            natives_dir: PathBuf::from("/cache/natives"),
            assets_dir: PathBuf::from("/cache/assets"),
            assets_index: "5".into(),
            game_dir: PathBuf::from("/instances/dev/game"),
            auth: AuthSession {
                username: "Steve".into(),
                uuid: "0123456789abcdef0123456789abcdef".into(),
                access_token: "token".into(),
                user_type: "msa".into(),
                xuid: String::new(),
                client_id: String::new(),
            },
            java: JavaInstall {
                path: PathBuf::from("/usr/bin/java"),
                major: 17,
                version: "17.0.20".into(),
                source: "test".into(),
            },
            memory_mb: 2048,
            extra_jvm: Vec::new(),
            extra_game: Vec::new(),
            quick_play: None,
            width: None,
            height: None,
        }
    }

    #[test]
    fn modern_arguments_are_expanded() {
        let version = version(
            vec![
                crate::versions::Argument::Plain("-Djava.library.path=${natives_directory}".into()),
                crate::versions::Argument::Plain("-cp".into()),
                crate::versions::Argument::Plain("${classpath}".into()),
            ],
            vec![
                crate::versions::Argument::Plain("--username".into()),
                crate::versions::Argument::Plain("${auth_player_name}".into()),
                crate::versions::Argument::Plain("--gameDir".into()),
                crate::versions::Argument::Plain("${game_directory}".into()),
            ],
        );
        let spec = build(&opts(version)).unwrap();
        assert_eq!(spec.program, PathBuf::from("/usr/bin/java"));
        assert!(spec.args.contains(&"-Xmx2048M".to_string()));
        assert!(spec.args.contains(&"/cache/1.20.4.jar:/cache/lib.jar".to_string()));
        assert!(spec
            .args
            .windows(2)
            .any(|w| w == ["--username", "Steve"]));
        assert!(spec
            .args
            .windows(2)
            .any(|w| w == ["--gameDir", "/instances/dev/game"]));
    }

    #[test]
    fn quick_play_feature_and_args() {
        let feature_rule = crate::versions::Rule {
            action: crate::versions::Action::Allow,
            os: None,
            features: Some(HashMap::from([("is_quick_play_multiplayer".into(), true)])),
        };
        let version = version(
            Vec::new(),
            vec![crate::versions::Argument::Conditional {
                rules: Some(vec![feature_rule]),
                value: crate::versions::StringOrVec::Many(vec![
                    "--quickPlayMultiplayer".into(),
                    "${quickPlayMultiplayer}".into(),
                ]),
            }],
        );
        let mut options = opts(version);
        options.quick_play = Some(QuickPlay::Multiplayer("mc.example.net:25565".into()));
        let spec = build(&options).unwrap();
        assert!(spec
            .args
            .windows(2)
            .any(|w| w == ["--quickPlayMultiplayer", "mc.example.net:25565"]));
    }

    #[test]
    fn legacy_server_fallback() {
        let version = version(
            Vec::new(),
            vec![crate::versions::Argument::Plain("--username".into())],
        );
        let mut options = opts(version);
        options.quick_play = Some(QuickPlay::Multiplayer("mc.example.net:25565".into()));
        let spec = build(&options).unwrap();
        assert!(spec.args.windows(2).any(|w| w == ["--server", "mc.example.net"]));
        assert!(spec.args.windows(2).any(|w| w == ["--port", "25565"]));
    }

    #[test]
    fn crash_diagnosis() {
        let log = "java.lang.OutOfMemoryError: Java heap space";
        let report = diagnose(Some(1), log).unwrap();
        assert_eq!(report.kind, CrashKind::OutOfMemory);
        assert!(!report.hints.is_empty());

        let log = "---- Minecraft Crash Report ----\nDescription: Rendering overlay";
        assert_eq!(diagnose(Some(1), log).unwrap().kind, CrashKind::Mod);
        assert!(diagnose(Some(0), "anything").is_none());
    }

    #[test]
    fn address_splitting() {
        assert_eq!(split_address("host:25566"), ("host".into(), 25566));
        assert_eq!(split_address("host"), ("host".into(), 25565));
    }

    #[test]
    fn measures_classpath_entries() {
        let version = version(
            vec![
                crate::versions::Argument::Plain("-cp".into()),
                crate::versions::Argument::Plain("${classpath}".into()),
            ],
            vec![crate::versions::Argument::Plain("--username".into())],
        );
        let spec = build(&opts(version)).unwrap();
        assert_eq!(spec.classpath_len(), 2);
    }

    // Keep the compiler honest about unused imports in test builds.
    #[allow(dead_code)]
    fn _artifact() -> Artifact {
        Artifact {
            path: None,
            sha1: None,
            size: None,
            url: String::new(),
        }
    }
}
