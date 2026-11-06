//! The per-instance mod manifest, on-disk scanning and safe file operations.
//!
//! Kestrel never deletes a jar permanently: removals move the file into
//! `.kestrel/trash/<timestamp>/`, and replaced jars are kept in
//! `.kestrel/backups/<transaction>/` so an install can be rolled back.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use sha1::{Digest as _, Sha1};
use sha2::Sha512;

use crate::bench;
use crate::content::{loader_key, ModProvider, ModUpdate};
use crate::error::{Error, Result};
use crate::instances::InstancePaths;
use crate::state::Loader;

pub const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstalledMod {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub version_id: String,
    pub version_number: String,
    pub file_name: String,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
    pub size: u64,
    pub installed_at: u64,
    pub feature_id: Option<String>,
    /// `modrinth` or `local`.
    pub source: String,
    pub enabled: bool,
    pub present: bool,
    pub icon_url: Option<String>,
    pub author: String,
    pub license: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModManifest {
    pub version: u32,
    pub mods: Vec<InstalledMod>,
}

impl Default for ModManifest {
    fn default() -> Self {
        Self {
            version: MANIFEST_VERSION,
            mods: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupMove {
    pub backup: String,
    pub original: String,
}

/// Written before files enter `mods/`. A crash leaves `committed: false`, and
/// `recover` restores the previous state on the next launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionJournal {
    pub committed: bool,
    pub targets: Vec<String>,
    pub backups: Vec<BackupMove>,
}

pub fn manifest_path(instance: &InstancePaths) -> PathBuf {
    instance.root.join(".kestrel").join("mods.json")
}

pub fn staging_root(instance: &InstancePaths) -> PathBuf {
    instance.root.join(".kestrel").join("staging")
}

pub fn backups_root(instance: &InstancePaths) -> PathBuf {
    instance.root.join(".kestrel").join("backups")
}

pub fn trash_root(instance: &InstancePaths) -> PathBuf {
    instance.root.join(".kestrel").join("trash")
}

pub fn load_manifest(instance: &InstancePaths) -> Result<ModManifest> {
    let path = manifest_path(instance);
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| Error::json(path.display().to_string(), e))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(ModManifest::default()),
        Err(err) => Err(Error::io(&path, err)),
    }
}

pub fn save_manifest(instance: &InstancePaths, manifest: &ModManifest) -> Result<()> {
    let path = manifest_path(instance);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|e| Error::json("mods manifest", e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| Error::io(&tmp, e))?;
    std::fs::rename(&tmp, &path).map_err(|e| Error::io(&path, e))?;
    Ok(())
}

/// Reject anything that could escape the instance directory.
pub fn validate_file_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 240 {
        return Err(Error::msg("invalid mod file name"));
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(Error::msg(format!("unsafe mod file name: {name}")));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(Error::msg("mod file name contains control characters"));
    }
    Ok(())
}

fn active_path(instance: &InstancePaths, file_name: &str) -> PathBuf {
    instance.mods.join(file_name)
}

fn disabled_path(instance: &InstancePaths, file_name: &str) -> PathBuf {
    instance.mods.join(format!("{file_name}.disabled"))
}

/// The file name as stored in the manifest always ends in `.jar`; the on-disk
/// name gains `.disabled` while a mod is switched off.
pub fn disk_path(instance: &InstancePaths, entry: &InstalledMod) -> PathBuf {
    if entry.enabled {
        active_path(instance, &entry.file_name)
    } else {
        disabled_path(instance, &entry.file_name)
    }
}

/// Merge the manifest with jars that were dropped into `mods/` by the user.
pub fn scan(instance: &InstancePaths) -> Result<Vec<InstalledMod>> {
    let mut manifest = load_manifest(instance)?;
    let mut known: HashSet<String> = HashSet::new();
    let mut out = Vec::new();

    for entry in manifest.mods.iter_mut() {
        if validate_file_name(&entry.file_name).is_err() {
            continue;
        }
        let active = active_path(instance, &entry.file_name);
        let disabled = disabled_path(instance, &entry.file_name);
        entry.enabled = active.exists();
        entry.present = active.exists() || disabled.exists();
        if entry.present {
            known.insert(entry.file_name.clone());
            if entry.size == 0 {
                if let Ok(meta) = std::fs::metadata(if entry.enabled { &active } else { &disabled })
                {
                    entry.size = meta.len();
                }
            }
            out.push(entry.clone());
        }
    }

    if let Ok(read_dir) = std::fs::read_dir(&instance.mods) {
        for item in read_dir.flatten() {
            let name = item.file_name().to_string_lossy().to_string();
            let (file_name, enabled) = if let Some(base) = name.strip_suffix(".jar.disabled") {
                (format!("{base}.jar"), false)
            } else if name.ends_with(".jar") {
                (name.clone(), true)
            } else {
                continue;
            };
            if known.contains(&file_name) || validate_file_name(&file_name).is_err() {
                continue;
            }
            let meta = item.metadata().ok();
            out.push(InstalledMod {
                title: title_from_file_name(&file_name),
                file_name,
                enabled,
                present: true,
                source: "local".into(),
                size: meta.map(|m| m.len()).unwrap_or(0),
                ..InstalledMod::default()
            });
        }
    }

    out.sort_by_key(|a| a.title.to_lowercase());
    Ok(out)
}

pub fn title_from_file_name(name: &str) -> String {
    let stem = name.strip_suffix(".disabled").unwrap_or(name);
    stem.strip_suffix(".jar").unwrap_or(stem).to_string()
}

pub fn set_enabled(instance: &InstancePaths, file_name: &str, enabled: bool) -> Result<()> {
    validate_file_name(file_name)?;
    let active = active_path(instance, file_name);
    let disabled = disabled_path(instance, file_name);

    if enabled {
        if disabled.exists() && !active.exists() {
            std::fs::rename(&disabled, &active).map_err(|e| Error::io(&active, e))?;
        } else if !active.exists() {
            return Err(Error::msg(format!("{file_name} is not installed")));
        }
    } else if active.exists() && !disabled.exists() {
        std::fs::rename(&active, &disabled).map_err(|e| Error::io(&disabled, e))?;
    } else if !disabled.exists() {
        return Err(Error::msg(format!("{file_name} is not installed")));
    }

    let mut manifest = load_manifest(instance)?;
    let mut changed = false;
    for entry in &mut manifest.mods {
        if entry.file_name == file_name {
            entry.enabled = enabled;
            entry.present = true;
            changed = true;
        }
    }
    if changed {
        save_manifest(instance, &manifest)?;
    }
    Ok(())
}

pub fn remove(instance: &InstancePaths, file_name: &str) -> Result<()> {
    validate_file_name(file_name)?;
    let active = active_path(instance, file_name);
    let disabled = disabled_path(instance, file_name);
    let source = if active.exists() {
        active
    } else if disabled.exists() {
        disabled
    } else {
        return Err(Error::msg(format!("{file_name} is not installed")));
    };

    let trash = trash_root(instance).join(bench::now_epoch_ms().to_string());
    std::fs::create_dir_all(&trash).map_err(|e| Error::io(&trash, e))?;
    let dest = trash.join(source.file_name().unwrap_or_default());
    std::fs::rename(&source, &dest).map_err(|e| Error::io(&dest, e))?;

    let mut manifest = load_manifest(instance)?;
    manifest.mods.retain(|m| m.file_name != file_name);
    save_manifest(instance, &manifest)?;
    Ok(())
}

/// Import a local jar. The file is validated as a zip, copied into `mods/` and
/// recorded as a local (not Modrinth-managed) entry.
pub fn import_local(instance: &InstancePaths, source: &Path) -> Result<InstalledMod> {
    let source = source.canonicalize().map_err(|e| Error::io(source, e))?;
    let meta = std::fs::metadata(&source).map_err(|e| Error::io(&source, e))?;
    if !meta.is_file() {
        return Err(Error::msg("the selected path is not a file"));
    }
    if meta.len() == 0 || meta.len() > 512 * 1024 * 1024 {
        return Err(Error::msg("the selected file has an invalid size"));
    }
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| Error::msg("the selected file has no name"))?;
    if !name.to_lowercase().ends_with(".jar") {
        return Err(Error::msg("only .jar mods can be imported"));
    }
    validate_file_name(&name)?;
    validate_jar(&source)?;

    instance.ensure()?;
    let target = active_path(instance, &name);
    let backup = if target.exists() {
        let backup_dir = backups_root(instance).join(format!("import-{}", bench::now_epoch_ms()));
        std::fs::create_dir_all(&backup_dir).map_err(|e| Error::io(&backup_dir, e))?;
        let saved = backup_dir.join(&name);
        std::fs::rename(&target, &saved).map_err(|e| Error::io(&saved, e))?;
        Some(saved)
    } else {
        None
    };

    let tmp = instance.mods.join(format!("{name}.kestrel-import"));
    let copy = std::fs::copy(&source, &tmp)
        .map_err(|e| Error::io(&tmp, e))
        .and_then(|_| std::fs::rename(&tmp, &target).map_err(|e| Error::io(&target, e)));
    if let Err(err) = copy {
        let _ = std::fs::remove_file(&tmp);
        if let Some(saved) = backup {
            let _ = std::fs::rename(saved, &target);
        }
        return Err(err);
    }

    let entry = InstalledMod {
        title: title_from_file_name(&name),
        file_name: name,
        sha1: Some(sha1_file(&target)?),
        size: meta.len(),
        installed_at: bench::now_epoch_ms(),
        source: "local".into(),
        enabled: true,
        present: true,
        ..InstalledMod::default()
    };

    let mut manifest = load_manifest(instance)?;
    manifest.mods.retain(|m| m.file_name != entry.file_name);
    manifest.mods.push(entry.clone());
    save_manifest(instance, &manifest)?;
    Ok(entry)
}

pub async fn check_updates(
    provider: &dyn ModProvider,
    instance: &InstancePaths,
    mc_version: &str,
    loader: Loader,
) -> Result<Vec<ModUpdate>> {
    let Some(loader_key) = loader_key(loader) else {
        return Ok(Vec::new());
    };
    let _ = loader_key;
    let manifest = load_manifest(instance)?;
    let candidates: Vec<InstalledMod> = manifest
        .mods
        .into_iter()
        .filter(|m| m.source == "modrinth" && !m.project_id.is_empty())
        .collect();

    let updates = stream::iter(candidates.into_iter().map(|entry| async move {
        let versions = provider
            .versions(&entry.project_id, mc_version, loader)
            .await
            .unwrap_or_default();
        let latest = versions.first()?;
        if latest.id == entry.version_id {
            return None;
        }
        Some(ModUpdate {
            project_id: entry.project_id.clone(),
            slug: entry.slug.clone(),
            title: entry.title.clone(),
            current_version: entry.version_number.clone(),
            current_version_id: entry.version_id.clone(),
            latest_version: latest.version_number.clone(),
            latest_version_id: latest.id.clone(),
            file_name: entry.file_name.clone(),
            icon_url: entry.icon_url.clone(),
        })
    }))
    .buffer_unordered(6)
    .filter_map(|item| async move { item })
    .collect::<Vec<_>>()
    .await;

    let mut updates = updates;
    updates.sort_by_key(|a| a.title.to_lowercase());
    Ok(updates)
}

/// Undo any install that was interrupted between staging and commit.
pub fn recover(instance: &InstancePaths) -> Result<()> {
    let staging = staging_root(instance);
    let Ok(read_dir) = std::fs::read_dir(&staging) else {
        return Ok(());
    };
    for item in read_dir.flatten() {
        let dir = item.path();
        if !dir.is_dir() {
            continue;
        }
        let journal_path = dir.join("journal.json");
        let Ok(bytes) = std::fs::read(&journal_path) else {
            let _ = std::fs::remove_dir_all(&dir);
            continue;
        };
        let Ok(journal) = serde_json::from_slice::<TransactionJournal>(&bytes) else {
            let _ = std::fs::remove_dir_all(&dir);
            continue;
        };
        if !journal.committed {
            rollback_journal(instance, &journal);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    Ok(())
}

/// Undo the file moves recorded in a transaction journal.
///
/// Backups are restored before targets are removed. A backup entry whose file
/// was never moved means the original is still in place, so that path is
/// preserved instead of being deleted as a freshly installed target.
pub fn rollback_journal(instance: &InstancePaths, journal: &TransactionJournal) {
    let mut preserved: HashSet<PathBuf> = HashSet::new();

    for backup in &journal.backups {
        let original = instance.root.join(&backup.original);
        let saved = instance.root.join(&backup.backup);
        if !original.starts_with(&instance.root) || !saved.starts_with(&instance.root) {
            continue;
        }
        if saved.exists() {
            if original.exists() {
                let _ = std::fs::remove_file(&original);
            }
            if let Some(parent) = original.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::rename(&saved, &original).is_ok() {
                preserved.insert(original);
            }
        } else {
            // The journal recorded this backup but the move never happened.
            preserved.insert(original);
        }
    }

    for target in &journal.targets {
        let path = instance.root.join(target);
        if path.starts_with(&instance.root) && !preserved.contains(&path) && path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

pub fn validate_jar(path: &Path) -> Result<()> {
    let file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| Error::msg(format!("{} is not a valid jar: {e}", path.display())))?;
    if archive.is_empty() {
        return Err(Error::msg(format!("{} is an empty jar", path.display())));
    }
    for index in 0..archive.len().min(4096) {
        let entry = archive
            .by_index(index)
            .map_err(|e| Error::msg(format!("{}: {e}", path.display())))?;
        let name = entry.name();
        if name.contains("..") || name.starts_with('/') || name.contains('\\') {
            return Err(Error::msg(format!(
                "{} contains an unsafe path: {name}",
                path.display()
            )));
        }
    }
    Ok(())
}

pub fn sha1_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buf).map_err(|e| Error::io(path, e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn sha512_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut hasher = Sha512::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buf).map_err(|e| Error::io(path, e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal() {
        assert!(validate_file_name("../evil.jar").is_err());
        assert!(validate_file_name("a/b.jar").is_err());
        assert!(validate_file_name("a\\b.jar").is_err());
        assert!(validate_file_name("ok-mod.jar").is_ok());
    }

    #[test]
    fn title_strips_jar_and_disabled_suffix() {
        assert_eq!(title_from_file_name("zoomify-1.0.jar"), "zoomify-1.0");
        assert_eq!(
            title_from_file_name("zoomify-1.0.jar.disabled"),
            "zoomify-1.0"
        );
    }

    #[test]
    fn manifest_round_trip() {
        let dir = std::env::temp_dir().join(format!("kestrel-mods-test-{}", bench::now_epoch_ms()));
        let instance = InstancePaths::new("_test");
        // Use a unique root without touching the real data directory.
        let instance = InstancePaths {
            root: dir.clone(),
            game_dir: dir.join("game"),
            mods: dir.join("game").join("mods"),
            resourcepacks: dir.join("game").join("resourcepacks"),
            shaderpacks: dir.join("game").join("shaderpacks"),
            saves: dir.join("game").join("saves"),
            config: dir.join("game").join("config"),
            logs: dir.join("logs"),
            natives: dir.join("natives"),
            ..instance
        };
        instance.ensure().unwrap();
        let manifest = ModManifest {
            version: MANIFEST_VERSION,
            mods: vec![InstalledMod {
                title: "Test".into(),
                file_name: "test.jar".into(),
                enabled: true,
                present: true,
                ..InstalledMod::default()
            }],
        };
        save_manifest(&instance, &manifest).unwrap();
        let loaded = load_manifest(&instance).unwrap();
        assert_eq!(loaded.mods.len(), 1);
        assert_eq!(loaded.mods[0].file_name, "test.jar");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn temp_instance(tag: &str) -> InstancePaths {
        let dir = std::env::temp_dir().join(format!(
            "kestrel-{tag}-{}-{}",
            std::process::id(),
            bench::now_epoch_ms()
        ));
        let base = InstancePaths::new("_test");
        InstancePaths {
            root: dir.clone(),
            game_dir: dir.join("game"),
            mods: dir.join("game").join("mods"),
            resourcepacks: dir.join("game").join("resourcepacks"),
            shaderpacks: dir.join("game").join("shaderpacks"),
            saves: dir.join("game").join("saves"),
            config: dir.join("game").join("config"),
            logs: dir.join("logs"),
            natives: dir.join("natives"),
            ..base
        }
    }

    #[test]
    fn rollback_preserves_unmoved_original_and_restores_backup() {
        let instance = temp_instance("rollback");
        instance.ensure().unwrap();
        let target = instance.mods.join("a.jar");
        std::fs::write(&target, b"old").unwrap();
        let journal = TransactionJournal {
            committed: false,
            targets: vec!["game/mods/a.jar".into()],
            backups: vec![BackupMove {
                backup: ".kestrel/backups/txn/a.jar".into(),
                original: "game/mods/a.jar".into(),
            }],
        };

        // The journal recorded the backup but the move never happened. The
        // original file must not be deleted as if it were a new target.
        rollback_journal(&instance, &journal);
        assert_eq!(std::fs::read(&target).unwrap(), b"old");

        // Simulate a partial commit: original moved aside, new jar in place.
        let backup = instance.root.join(".kestrel/backups/txn/a.jar");
        std::fs::create_dir_all(backup.parent().unwrap()).unwrap();
        std::fs::rename(&target, &backup).unwrap();
        std::fs::write(&target, b"new").unwrap();
        rollback_journal(&instance, &journal);
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert!(!backup.exists());

        let _ = std::fs::remove_dir_all(&instance.root);
    }

    #[test]
    fn rollback_removes_new_targets_without_backup() {
        let instance = temp_instance("rollback-new");
        instance.ensure().unwrap();
        let target = instance.mods.join("new.jar");
        std::fs::write(&target, b"new").unwrap();
        let journal = TransactionJournal {
            committed: false,
            targets: vec!["game/mods/new.jar".into()],
            backups: Vec::new(),
        };
        rollback_journal(&instance, &journal);
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(&instance.root);
    }
}
