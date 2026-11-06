//! Content discovery and installation.
//!
//! Modrinth is the primary provider. The rest of the launcher only talks to the
//! types in this module, so a CurseForge adapter can be added later without
//! changing the resolver, the installer or the UI.

pub mod catalog;
pub mod install;
pub mod modrinth;
pub mod resolve;

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::mods::InstalledMod;
use crate::state::Loader;

/// A project as shown in search results and on cards.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectSummary {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub icon_url: Option<String>,
    pub categories: Vec<String>,
    pub loaders: Vec<String>,
    pub game_versions: Vec<String>,
    pub downloads: u64,
    pub license: String,
    pub client_side: String,
    pub latest_version: Option<String>,
    pub featured: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GalleryImage {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
}

/// Full project details, loaded only when a card is opened.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectDetail {
    #[serde(flatten)]
    pub summary: ProjectSummary,
    pub body: String,
    pub gallery: Vec<GalleryImage>,
    pub source_url: Option<String>,
    pub issues_url: Option<String>,
    pub license_name: String,
    pub license_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModDependency {
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub dependency_type: String,
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModFile {
    pub filename: String,
    pub url: String,
    pub sha1: String,
    pub sha512: String,
    pub size: u64,
    pub primary: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModVersion {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    pub version_type: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub dependencies: Vec<ModDependency>,
    pub files: Vec<ModFile>,
    pub date_published: String,
}

impl ModVersion {
    /// Modrinth marks one file as primary; fall back to the first jar.
    pub fn primary_file(&self) -> Option<&ModFile> {
        self.files
            .iter()
            .find(|f| f.primary && f.filename.ends_with(".jar"))
            .or_else(|| self.files.iter().find(|f| f.filename.ends_with(".jar")))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchResult {
    pub hits: Vec<ProjectSummary>,
    pub offset: u32,
    pub limit: u32,
    pub total_hits: u32,
}

/// What the user asked to install. `versionId` pins a version (used by the
/// Kestrel Essentials cards); otherwise the resolver picks the newest
/// compatible release.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstallItem {
    pub project_id: String,
    pub version_id: Option<String>,
    pub feature_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlannedMod {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub author: String,
    pub icon_url: Option<String>,
    pub version_id: String,
    pub version_number: String,
    pub filename: String,
    pub url: String,
    pub sha1: String,
    pub sha512: String,
    pub size: u64,
    pub license: String,
    pub feature_id: Option<String>,
    pub is_dependency: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstallPlan {
    pub instance_id: String,
    pub mc_version: String,
    pub loader: Loader,
    pub items: Vec<PlannedMod>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstallProgress {
    pub transaction_id: String,
    pub phase: String,
    pub completed_files: usize,
    pub total_files: usize,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub current: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstallOutcome {
    pub transaction_id: String,
    pub instance_id: String,
    pub installed: Vec<InstalledMod>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModUpdate {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub current_version: String,
    pub current_version_id: String,
    pub latest_version: String,
    pub latest_version_id: String,
    pub file_name: String,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EssentialFeature {
    pub id: String,
    pub name: String,
    pub description: String,
    pub keybind: String,
    pub category: String,
    pub essential: bool,
    pub project: Option<ProjectSummary>,
    pub version: Option<ModVersion>,
    pub available: bool,
    pub reason: Option<String>,
    pub installed: Option<InstalledMod>,
    pub update: Option<ModUpdate>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstanceSummary {
    pub id: String,
    pub name: String,
    pub mc_version: String,
    pub loader: Loader,
    pub loader_version: Option<String>,
    pub game_dir: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModsCatalog {
    pub instance: Option<InstanceSummary>,
    pub loader_ready: bool,
    pub loader_error: Option<String>,
    pub features: Vec<EssentialFeature>,
    pub installed: Vec<InstalledMod>,
    pub updates: Vec<ModUpdate>,
    pub warnings: Vec<String>,
}

/// Async provider interface. Kept object-safe with boxed futures so tests can
/// use an in-memory provider without pulling in an async-trait dependency.
pub trait ModProvider: Send + Sync {
    fn project<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<ProjectDetail>> + Send + 'a>>;

    fn versions<'a>(
        &'a self,
        id: &'a str,
        mc_version: &'a str,
        loader: Loader,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ModVersion>>> + Send + 'a>>;
}

/// Map Kestrel's loader enum to Modrinth's loader identifier.
pub fn loader_key(loader: Loader) -> Option<&'static str> {
    loader_keys(loader).first().copied()
}

/// Every Modrinth loader tag that can satisfy this instance. Quilt runs Fabric
/// mods, so a Quilt instance accepts both tags (Modrinth only lists `quilt` when
/// the author explicitly marks a build as Quilt-compatible).
pub fn loader_keys(loader: Loader) -> &'static [&'static str] {
    match loader {
        Loader::Fabric => &["fabric"],
        Loader::Quilt => &["quilt", "fabric"],
        Loader::Forge => &["forge"],
        Loader::NeoForge => &["neoforge"],
        Loader::Vanilla => &[],
    }
}
