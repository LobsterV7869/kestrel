use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{error::Result, paths};

pub const SCHEMA: u32 = 1;

/// Everything the UI needs for an instant first paint. Kept deliberately small
/// and read synchronously at startup; never contains secrets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppState {
    pub schema: u32,
    pub profiles: Vec<Profile>,
    pub accounts: Vec<AccountSummary>,
    pub selected_profile: Option<String>,
    pub last_server: Option<String>,
    pub updated_at: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            profiles: Vec::new(),
            accounts: Vec::new(),
            selected_profile: None,
            last_server: None,
            updated_at: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    #[default]
    Vanilla,
    Fabric,
    Quilt,
    NeoForge,
    Forge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub mc_version: String,
    pub loader: Loader,
    pub loader_version: Option<String>,
    pub java_path: Option<String>,
    /// `None` means "choose automatically from system RAM".
    pub memory_mb: Option<u32>,
    pub jvm_args: Vec<String>,
    pub game_args: Vec<String>,
    pub icon: Option<String>,
    /// Content preset this profile was created from.
    pub preset: Preset,
    pub created_at: u64,
    pub last_played: Option<u64>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "New profile".into(),
            mc_version: String::new(),
            loader: Loader::Vanilla,
            loader_version: None,
            java_path: None,
            memory_mb: None,
            jvm_args: Vec::new(),
            game_args: Vec::new(),
            icon: None,
            preset: Preset::Optimized,
            created_at: 0,
            last_played: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    #[default]
    Optimized,
    Vanilla,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountSummary {
    pub id: String,
    pub username: String,
    pub uuid: String,
    pub kind: AccountKind,
}

impl Default for AccountSummary {
    fn default() -> Self {
        Self {
            id: String::new(),
            username: String::new(),
            uuid: String::new(),
            kind: AccountKind::Microsoft,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    #[default]
    Microsoft,
}

impl AppState {
    /// Fast path used before first paint. A missing or corrupt file yields an
    /// empty state rather than blocking the UI on an error dialog.
    pub fn load_or_default() -> Self {
        match Self::load(&paths::state_file()) {
            Ok(state) => state,
            Err(err) => {
                tracing::debug!(target: "state", "no usable cached state: {err}");
                Self::default()
            }
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| crate::error::Error::io(path, e))?;
        let mut state: Self = serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::Error::json(path.display().to_string(), e))?;
        if state.schema == 0 {
            state.schema = SCHEMA;
        }
        Ok(state)
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&paths::state_file())
    }

    /// Atomic write: temp file in the same directory, fsync, rename.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::error::Error::io(parent, e))?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| crate::error::Error::json("state", e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| crate::error::Error::io(&tmp, e))?;
        std::fs::rename(&tmp, path).map_err(|e| crate::error::Error::io(path, e))?;
        Ok(())
    }

    pub fn selected(&self) -> Option<&Profile> {
        let id = self.selected_profile.as_deref()?;
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn touch(&mut self) {
        self.updated_at = crate::bench::now_epoch_ms();
    }
}
