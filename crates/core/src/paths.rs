use std::path::PathBuf;

use crate::error::{Error, Result};

/// Root application home. Overridable with `KESTREL_HOME` for tests and for
/// portable installs.
pub fn home() -> PathBuf {
    if let Ok(home) = std::env::var("KESTREL_HOME") {
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kestrel")
}

pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("KESTREL_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kestrel")
}

pub fn cache_dir() -> PathBuf {
    home().join("cache")
}

pub fn logs_dir() -> PathBuf {
    home().join("logs")
}

/// Shared, content-addressed store. Profiles link into this rather than
/// duplicating jars and assets.
pub fn shared_dir() -> PathBuf {
    home().join("shared")
}

pub fn shared_versions_dir() -> PathBuf {
    shared_dir().join("versions")
}

pub fn shared_libraries_dir() -> PathBuf {
    shared_dir().join("libraries")
}

pub fn shared_assets_dir() -> PathBuf {
    shared_dir().join("assets")
}

pub fn shared_indexes_dir() -> PathBuf {
    shared_assets_dir().join("indexes")
}

pub fn shared_objects_dir() -> PathBuf {
    shared_assets_dir().join("objects")
}

pub fn shared_mods_dir() -> PathBuf {
    shared_dir().join("mods")
}

pub fn shared_java_dir() -> PathBuf {
    shared_dir().join("java")
}

pub fn instances_dir() -> PathBuf {
    home().join("instances")
}

pub fn instance_dir(id: &str) -> PathBuf {
    instances_dir().join(id)
}

pub fn state_file() -> PathBuf {
    config_dir().join("state.json")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.json")
}

pub fn ensure_dirs() -> Result<()> {
    for dir in [
        home(),
        config_dir(),
        cache_dir(),
        logs_dir(),
        shared_versions_dir(),
        shared_libraries_dir(),
        shared_indexes_dir(),
        shared_objects_dir(),
        shared_mods_dir(),
        shared_java_dir(),
        instances_dir(),
    ] {
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    }
    Ok(())
}
