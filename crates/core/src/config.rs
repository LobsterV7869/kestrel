use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{error::Result, paths};

/// User-visible configuration. Secrets (tokens, CurseForge keys) live in the
/// system keyring, never here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// Azure application (client) ID used for Microsoft OAuth. Public by
    /// nature; can also be supplied via `KESTREL_AZURE_CLIENT_ID`.
    pub azure_client_id: String,
    /// `None` = derive from system RAM at launch time.
    pub default_memory_mb: Option<u32>,
    /// Extra JVM flags applied to every profile.
    pub global_jvm_args: Vec<String>,
    pub auto_join_last_server: bool,
    pub enable_appcds: bool,
    pub prewarm_page_cache: bool,
    pub close_to_tray: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            azure_client_id: "965848f0-57e1-49d6-817e-9e9e8d5bf932".to_string(),
            default_memory_mb: None,
            global_jvm_args: Vec::new(),
            auto_join_last_server: false,
            enable_appcds: true,
            prewarm_page_cache: true,
            close_to_tray: false,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let mut config = Self::load_from(&paths::config_file()).unwrap_or_default();
        if let Ok(id) = std::env::var("KESTREL_AZURE_CLIENT_ID") {
            if !id.trim().is_empty() {
                config.azure_client_id = id.trim().to_string();
            }
        }
        config
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| crate::error::Error::io(path, e))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::Error::json(path.display().to_string(), e))
    }

    pub fn save(&self) -> Result<()> {
        let path = paths::config_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::error::Error::io(parent, e))?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| crate::error::Error::json("config", e))?;
        std::fs::write(&path, bytes).map_err(|e| crate::error::Error::io(&path, e))
    }

    pub fn client_id(&self) -> Result<&str> {
        if self.azure_client_id.trim().is_empty() {
            Err(crate::error::Error::Config(
                "Azure client ID is not set. Put it in config.json or set KESTREL_AZURE_CLIENT_ID. \
                 See README for how to register an app."
                    .into(),
            ))
        } else {
            Ok(self.azure_client_id.trim())
        }
    }
}
