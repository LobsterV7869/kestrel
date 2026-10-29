//! Installing a vanilla Minecraft version into the shared cache.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cache::{self, DownloadSummary, SharedCache};
use crate::download::DownloadTask;
use crate::error::{Error, Result};
use crate::paths;
use crate::versions::{
    self, Artifact, AssetIndex, Extract, RuleContext, VersionJson, VersionManifest,
};

const MANIFEST_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone)]
pub struct InstalledVersion {
    pub id: String,
    pub json: VersionJson,
    pub jar: PathBuf,
    pub classpath: Vec<PathBuf>,
    pub natives_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub assets_index: String,
}

impl InstalledVersion {
    pub fn java_major(&self) -> u32 {
        self.json.java_major()
    }
}

/// Manifest with a 30-minute on-disk cache. A stale cache is still better than
/// no manifest when the network is unavailable.
pub async fn manifest() -> Result<VersionManifest> {
    let path = paths::cache_dir().join("version_manifest.json");
    let fresh = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .map(|age| age < MANIFEST_TTL)
        .unwrap_or(false);
    if fresh {
        if let Ok(manifest) = read_manifest(&path) {
            return Ok(manifest);
        }
    }
    match versions::fetch_manifest().await {
        Ok(manifest) => {
            let _ = write_manifest(&path, &manifest);
            Ok(manifest)
        }
        Err(err) => match read_manifest(&path) {
            Ok(manifest) => {
                tracing::warn!(target: "versions", "using cached manifest: {err}");
                Ok(manifest)
            }
            Err(_) => Err(err),
        },
    }
}

fn read_manifest(path: &Path) -> Result<VersionManifest> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| Error::json(path.display().to_string(), e))
}

fn write_manifest(path: &Path, manifest: &VersionManifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let bytes = serde_json::to_vec(manifest)
        .map_err(|e| Error::json("version manifest", e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| Error::io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))?;
    Ok(())
}

fn task(artifact: &Artifact, dest: PathBuf) -> Result<DownloadTask> {
    Ok(DownloadTask {
        url: artifact.url.clone(),
        dest,
        sha1: artifact.sha1.clone(),
        size: artifact.size,
    })
}

pub async fn install_version(id: &str, cache: &SharedCache) -> Result<InstalledVersion> {
    let manifest = manifest().await?;
    let entry = versions::find_version(&manifest, id)
        .ok_or_else(|| Error::msg(format!("unknown Minecraft version '{id}'")))?;

    let versions_root = paths::shared_versions_dir();
    let libraries_root = paths::shared_libraries_dir();
    let version_dir = versions_root.join(id);
    std::fs::create_dir_all(&version_dir).map_err(|e| Error::io(&version_dir, e))?;

    let json_path = version_dir.join(format!("{id}.json"));
    cache
        .ensure(DownloadTask {
            url: entry.url.clone(),
            dest: json_path.clone(),
            sha1: Some(entry.sha1.clone()),
            size: None,
        })
        .await?;
    let json: VersionJson = serde_json::from_slice(
        &std::fs::read(&json_path).map_err(|e| Error::io(&json_path, e))?,
    )
    .map_err(|e| Error::json(json_path.display().to_string(), e))?;

    let client = json
        .downloads
        .client
        .clone()
        .ok_or_else(|| Error::msg(format!("{id} has no client download")))?;
    let jar = version_dir.join(format!("{id}.jar"));
    cache.ensure(task(&client, jar.clone())?).await?;

    let ctx = RuleContext::current();
    let resolved = json.library_paths(&ctx, &libraries_root)?;

    let mut seen = HashSet::new();
    let mut tasks = Vec::new();
    for library in versions::resolve_libraries(&json.libraries, &ctx) {
        for artifact in [library.artifact, library.native].into_iter().flatten() {
            let relative = artifact.path()?;
            let dest = libraries_root.join(relative);
            if seen.insert(dest.clone()) {
                tasks.push(task(&artifact, dest)?);
            }
        }
    }
    let library_summary = cache.ensure_many(tasks, 24).await?;

    let natives_dir = version_dir.join(format!("natives-{}", ctx.platform.arch));
    extract_natives(&resolved.natives, &natives_dir)?;

    let mut assets_index = json
        .assets
        .clone()
        .unwrap_or_else(|| "legacy".to_string());
    let mut asset_summary = DownloadSummary::default();
    if let Some(index_ref) = &json.asset_index {
        assets_index = index_ref.id.clone();
        let index_path = paths::shared_indexes_dir().join(format!("{}.json", index_ref.id));
        cache
            .ensure(DownloadTask {
                url: index_ref.url.clone(),
                dest: index_path.clone(),
                sha1: Some(index_ref.sha1.clone()),
                size: Some(index_ref.size),
            })
            .await?;
        let index: AssetIndex = serde_json::from_slice(
            &std::fs::read(&index_path).map_err(|e| Error::io(&index_path, e))?,
        )
        .map_err(|e| Error::json(index_path.display().to_string(), e))?;

        let mut tasks = Vec::with_capacity(index.objects.len());
        for object in index.objects.values() {
            if object.hash.len() < 2 {
                continue;
            }
            let dest = cache::object_path(&object.hash);
            tasks.push(DownloadTask {
                url: cache::asset_url(&object.hash),
                dest,
                sha1: Some(object.hash.clone()),
                size: Some(object.size),
            });
        }
        asset_summary = cache.ensure_many(tasks, 32).await?;
    }

    cache.save()?;

    tracing::info!(
        target: "versions",
        "installed {id}: {} libraries ({} fetched), {} assets ({} fetched)",
        resolved.classpath.len(),
        library_summary.downloaded,
        asset_summary.downloaded + asset_summary.cached,
        asset_summary.downloaded
    );

    Ok(InstalledVersion {
        id: id.to_string(),
        json,
        jar,
        classpath: resolved.classpath,
        natives_dir,
        assets_dir: paths::shared_assets_dir(),
        assets_index,
    })
}

fn extract_natives(
    natives: &[(Artifact, Option<Extract>)],
    dest: &Path,
) -> Result<()> {
    if natives.is_empty() {
        std::fs::create_dir_all(dest).map_err(|e| Error::io(dest, e))?;
        return Ok(());
    }

    let mut key = String::new();
    for (artifact, _) in natives {
        key.push_str(artifact.sha1.as_deref().unwrap_or(&artifact.url));
        key.push('|');
    }
    let marker = dest.join(".kestrel-natives");
    if std::fs::read_to_string(&marker).ok().as_deref() == Some(key.as_str()) {
        return Ok(());
    }
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| Error::io(dest, e))?;
    }
    std::fs::create_dir_all(dest).map_err(|e| Error::io(dest, e))?;

    for (artifact, extract) in natives {
        let archive_path = paths::shared_libraries_dir().join(artifact.path()?);
        let file = std::fs::File::open(&archive_path).map_err(|e| Error::io(&archive_path, e))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| Error::msg(format!("{}: {e}", archive_path.display())))?;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|e| Error::msg(format!("{}: {e}", archive_path.display())))?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_string();
            if name.starts_with("META-INF/") {
                continue;
            }
            if let Some(extract) = extract {
                if extract.exclude.iter().any(|excluded| name.starts_with(excluded)) {
                    continue;
                }
            }
            let Some(relative) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
                continue;
            };
            let out = dest.join(relative);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            let mut output = std::fs::File::create(&out).map_err(|e| Error::io(&out, e))?;
            std::io::copy(&mut entry, &mut output).map_err(|e| Error::io(&out, e))?;
        }
    }

    std::fs::write(&marker, key).map_err(|e| Error::io(&marker, e))?;
    Ok(())
}
