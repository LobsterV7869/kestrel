//! Fabric and Quilt loader discovery and installation.
//!
//! Forge/NeoForge are represented in the data model but intentionally deferred:
//! their installers are a separate, much larger compatibility problem. The
//! Mods UI reports that clearly instead of guessing.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cache::SharedCache;
use crate::download::DownloadTask;
use crate::error::{Error, Result};
use crate::http;
use crate::paths;
use crate::state::Loader;
use crate::versions;

const FABRIC_META: &str = "https://meta.fabricmc.net/v2";
const QUILT_META: &str = "https://meta.quiltmc.org/v3";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderVersion {
    pub version: String,
    pub stable: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoaderStatus {
    pub ready: bool,
    pub installed_version: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstalledLoader {
    pub loader: Loader,
    pub mc_version: String,
    pub loader_version: String,
    pub profile_id: String,
    pub main_class: String,
    pub libraries: usize,
}

#[derive(Debug, Deserialize)]
struct RawFabricEntry {
    loader: RawFabricLoader,
}

#[derive(Debug, Deserialize)]
struct RawFabricLoader {
    version: String,
    #[serde(default)]
    stable: bool,
}

#[derive(Debug, Deserialize)]
struct RawQuiltEntry {
    loader: RawQuiltLoader,
}

#[derive(Debug, Deserialize)]
struct RawQuiltLoader {
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProfile {
    id: String,
    #[serde(default)]
    inherits_from: String,
    #[serde(default)]
    main_class: String,
    #[serde(default)]
    libraries: Vec<RawLibrary>,
}

#[derive(Debug, Deserialize)]
struct RawLibrary {
    name: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    sha1: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

pub fn supported(loader: Loader) -> bool {
    matches!(loader, Loader::Fabric | Loader::Quilt)
}

pub async fn list_versions(loader: Loader, mc_version: &str) -> Result<Vec<LoaderVersion>> {
    match loader {
        Loader::Fabric => {
            let url = format!("{FABRIC_META}/versions/loader/{mc_version}");
            let raw: Vec<RawFabricEntry> = http::get_json(&url).await?;
            let mut out: Vec<LoaderVersion> = raw
                .into_iter()
                .map(|entry| LoaderVersion {
                    version: entry.loader.version,
                    stable: entry.loader.stable,
                })
                .collect();
            out.sort_by_key(|a| std::cmp::Reverse(a.stable));
            Ok(out)
        }
        Loader::Quilt => {
            let url = format!("{QUILT_META}/versions/loader/{mc_version}");
            let raw: Vec<RawQuiltEntry> = http::get_json(&url).await?;
            let mut out: Vec<LoaderVersion> = raw
                .into_iter()
                .map(|entry| {
                    let stable = !is_prerelease(&entry.loader.version);
                    LoaderVersion {
                        version: entry.loader.version,
                        stable,
                    }
                })
                .collect();
            out.sort_by_key(|a| std::cmp::Reverse(a.stable));
            Ok(out)
        }
        Loader::Vanilla => Err(Error::msg("Vanilla does not need a mod loader.")),
        Loader::Forge | Loader::NeoForge => Err(Error::msg(
            "Forge and NeoForge loader installation is not supported yet. Use Fabric or Quilt.",
        )),
    }
}

pub fn status(loader: Loader, mc_version: &str, loader_version: Option<&str>) -> LoaderStatus {
    match loader {
        Loader::Vanilla => LoaderStatus {
            ready: false,
            installed_version: None,
            message: Some("This instance is vanilla. Install Fabric or Quilt first.".into()),
        },
        Loader::Forge | Loader::NeoForge => LoaderStatus {
            ready: false,
            installed_version: None,
            message: Some(
                "Forge and NeoForge are not supported yet. Create a Fabric or Quilt instance."
                    .into(),
            ),
        },
        Loader::Fabric | Loader::Quilt => {
            let Some(version) = loader_version.filter(|v| !v.trim().is_empty()) else {
                return LoaderStatus {
                    ready: false,
                    installed_version: None,
                    message: Some(format!(
                        "{} is selected, but its loader is not installed.",
                        loader_name(loader)
                    )),
                };
            };
            let id = profile_id(loader, mc_version, version);
            let path = profile_json_path(&id);
            if path.exists() {
                LoaderStatus {
                    ready: true,
                    installed_version: Some(version.to_string()),
                    message: None,
                }
            } else {
                LoaderStatus {
                    ready: false,
                    installed_version: None,
                    message: Some(format!(
                        "{} {version} is selected but its profile is missing. Reinstall the loader.",
                        loader_name(loader)
                    )),
                }
            }
        }
    }
}

pub async fn install(
    loader: Loader,
    mc_version: &str,
    requested_version: Option<&str>,
    cache: &SharedCache,
) -> Result<InstalledLoader> {
    if !supported(loader) {
        return Err(Error::msg(
            "Only Fabric and Quilt loader installation is supported in this build.",
        ));
    }
    if mc_version.trim().is_empty() {
        return Err(Error::msg("The instance has no Minecraft version."));
    }

    let versions = list_versions(loader, mc_version).await?;
    if versions.is_empty() {
        return Err(Error::msg(format!(
            "{} has no loader builds for Minecraft {mc_version}.",
            loader_name(loader)
        )));
    }
    let chosen = match requested_version {
        Some(requested) => versions
            .iter()
            .find(|v| v.version == requested)
            .cloned()
            .ok_or_else(|| {
                Error::msg(format!(
                    "{} {requested} is not available for Minecraft {mc_version}.",
                    loader_name(loader)
                ))
            })?,
        None => versions
            .iter()
            .find(|v| v.stable)
            .or_else(|| versions.first())
            .cloned()
            .unwrap(),
    };

    let profile_url = match loader {
        Loader::Fabric => format!(
            "{FABRIC_META}/versions/loader/{mc_version}/{}/profile/json",
            chosen.version
        ),
        Loader::Quilt => format!(
            "{QUILT_META}/versions/loader/{mc_version}/{}/profile/json",
            chosen.version
        ),
        _ => unreachable!(),
    };
    let bytes = http::get_bytes(&profile_url).await?;
    let profile: RawProfile =
        serde_json::from_slice(&bytes).map_err(|e| Error::json(profile_url.clone(), e))?;
    if !profile.inherits_from.is_empty() && profile.inherits_from != mc_version {
        return Err(Error::msg(format!(
            "{} returned a profile for Minecraft {}, not {mc_version}.",
            loader_name(loader),
            profile.inherits_from
        )));
    }

    let mut tasks = Vec::new();
    for library in &profile.libraries {
        let path = versions::maven_path(&library.name)?;
        let base = library
            .url
            .clone()
            .unwrap_or_else(|| versions::LIBRARY_BASE.to_string());
        let url = format!("{}/{path}", base.trim_end_matches('/'));
        let sha1 = match library.sha1.clone() {
            Some(sha1) if !sha1.is_empty() => Some(sha1),
            _ => fetch_sha1(&url).await,
        };
        let dest = paths::shared_libraries_dir().join(&path);
        tasks.push(DownloadTask {
            url,
            dest,
            sha1,
            size: library.size,
        });
    }
    let summary = cache.ensure_many(tasks, 12).await?;
    cache.save()?;

    let version_dir = paths::shared_versions_dir().join(&profile.id);
    std::fs::create_dir_all(&version_dir).map_err(|e| Error::io(&version_dir, e))?;
    let json_path = version_dir.join(format!("{}.json", profile.id));
    let tmp = json_path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| Error::io(&tmp, e))?;
    std::fs::rename(&tmp, &json_path).map_err(|e| Error::io(&json_path, e))?;

    tracing::info!(
        target: "loaders",
        "installed {} {} for {}: {} libraries ({} fetched)",
        loader_name(loader),
        chosen.version,
        mc_version,
        profile.libraries.len(),
        summary.downloaded
    );

    Ok(InstalledLoader {
        loader,
        mc_version: mc_version.to_string(),
        loader_version: chosen.version,
        profile_id: profile.id,
        main_class: profile.main_class,
        libraries: profile.libraries.len(),
    })
}

fn profile_json_path(profile_id: &str) -> PathBuf {
    paths::shared_versions_dir()
        .join(profile_id)
        .join(format!("{profile_id}.json"))
}

pub fn profile_id(loader: Loader, mc_version: &str, loader_version: &str) -> String {
    match loader {
        Loader::Fabric => format!("fabric-loader-{loader_version}-{mc_version}"),
        Loader::Quilt => format!("quilt-loader-{loader_version}-{mc_version}"),
        _ => format!(
            "{}-{loader_version}-{mc_version}",
            loader_name(loader).to_lowercase()
        ),
    }
}

pub fn loader_name(loader: Loader) -> &'static str {
    match loader {
        Loader::Vanilla => "Vanilla",
        Loader::Fabric => "Fabric",
        Loader::Quilt => "Quilt",
        Loader::NeoForge => "NeoForge",
        Loader::Forge => "Forge",
    }
}

fn is_prerelease(version: &str) -> bool {
    let lower = version.to_lowercase();
    lower.contains("beta") || lower.contains("alpha") || lower.contains("rc")
}

async fn fetch_sha1(url: &str) -> Option<String> {
    let bytes = tokio::time::timeout(
        Duration::from_secs(20),
        http::get_bytes(&format!("{url}.sha1")),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8(bytes.to_vec()).ok()?;
    let hash = text.split_whitespace().next()?.trim();
    if hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(hash.to_lowercase())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_profile_ids_match_meta() {
        assert_eq!(
            profile_id(Loader::Fabric, "1.21.1", "0.16.9"),
            "fabric-loader-0.16.9-1.21.1"
        );
        assert_eq!(
            profile_id(Loader::Quilt, "1.21.1", "0.27.0"),
            "quilt-loader-0.27.0-1.21.1"
        );
    }

    #[test]
    fn status_rejects_vanilla_and_unsupported() {
        assert!(!status(Loader::Vanilla, "1.21.1", None).ready);
        assert!(!status(Loader::NeoForge, "1.21.1", Some("1")).ready);
        assert!(!status(Loader::Fabric, "1.21.1", None).ready);
    }

    #[test]
    fn prerelease_detection() {
        assert!(is_prerelease("0.20.0-beta.9"));
        assert!(!is_prerelease("0.16.9"));
    }
}
