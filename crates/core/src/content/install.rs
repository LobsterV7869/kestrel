//! Transactional mod installation.
//!
//! Every file is downloaded to a per-transaction staging directory, checked
//! against Modrinth's SHA-1/SHA-512 and size, validated as a jar, and only then
//! moved into `mods/`. Existing jars are backed up first. A journal makes an
//! interrupted commit recoverable on the next launch.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use sha1::{Digest as _, Sha1};
use sha2::Sha512;
use tokio::io::AsyncWriteExt;

use crate::bench;
use crate::error::{Error, Result};
use crate::instances::InstancePaths;
use crate::mods::{self, backups_root, staging_root, BackupMove, InstalledMod, TransactionJournal};
use crate::state::Loader;

use super::resolve;
use super::{InstallOutcome, InstallPlan, InstallProgress, PlannedMod};

pub type ProgressFn = Arc<dyn Fn(InstallProgress) + Send + Sync>;

const DOWNLOAD_CONCURRENCY: usize = 4;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(80);

pub async fn execute(
    instance: &InstancePaths,
    plan: &InstallPlan,
    progress: ProgressFn,
) -> Result<InstallOutcome> {
    if plan.items.is_empty() {
        return Err(Error::msg("Nothing to install."));
    }
    instance.ensure()?;
    mods::recover(instance)?;

    let transaction_id = format!("{}-{}", bench::now_epoch_ms(), std::process::id());
    let staging = staging_root(instance).join(&transaction_id);
    std::fs::create_dir_all(&staging).map_err(|e| Error::io(&staging, e))?;

    let total_bytes = plan.items.iter().map(|item| item.size).sum();
    let emitter = Arc::new(ProgressEmitter {
        sink: progress,
        transaction_id: transaction_id.clone(),
        total_files: plan.items.len(),
        total_bytes,
        downloaded: Arc::new(AtomicU64::new(0)),
        completed: Arc::new(AtomicUsize::new(0)),
        last: Mutex::new(Instant::now() - PROGRESS_INTERVAL),
    });
    emitter.emit("downloading", None, Some("Downloading mods".into()), true);

    // Build the futures in a plain loop rather than an async closure so the
    // returned future has no higher-ranked lifetime that Tauri rejects.
    let mut futures = Vec::with_capacity(plan.items.len());
    for item in &plan.items {
        let staging = staging.clone();
        let emitter = emitter.clone();
        futures.push(async move {
            if !allowed_download_url(&item.url) {
                return Err(Error::msg(format!(
                    "Refusing to download '{}' from an untrusted host.",
                    item.filename
                )));
            }
            let dest = staging.join(&item.filename);
            download_one(item, &dest, &emitter).await?;
            emitter.completed.fetch_add(1, Ordering::Relaxed);
            emitter.emit("downloading", Some(item.filename.clone()), None, true);
            Ok(())
        });
    }
    let downloads = futures_util::stream::iter(futures)
        .buffer_unordered(DOWNLOAD_CONCURRENCY)
        .collect::<Vec<Result<()>>>()
        .await;

    if let Some(err) = downloads.into_iter().find_map(Result::err) {
        let _ = std::fs::remove_dir_all(&staging);
        emitter.emit("failed", None, Some(err.to_string()), true);
        return Err(err);
    }

    emitter.emit("verifying", None, Some("Verifying mod files".into()), true);
    let mut mod_ids: HashSet<String> = HashSet::new();
    for item in &plan.items {
        let path = staging.join(&item.filename);
        if let Some(mod_id) = validate_jar_metadata(&path, &plan.mc_version, plan.loader)? {
            if !mod_id.is_empty() && !mod_ids.insert(mod_id.clone()) {
                let err = Error::msg(format!(
                    "Two selected mods both declare the mod id '{mod_id}'. Refusing to install."
                ));
                let _ = std::fs::remove_dir_all(&staging);
                emitter.emit("failed", None, Some(err.to_string()), true);
                return Err(err);
            }
        }
    }

    emitter.emit(
        "committing",
        None,
        Some("Installing into the instance".into()),
        true,
    );
    match commit(instance, plan, &staging, &transaction_id) {
        Ok(installed) => {
            let _ = std::fs::remove_dir_all(&staging);
            emitter.emit("done", None, Some("Installed".into()), true);
            Ok(InstallOutcome {
                transaction_id,
                instance_id: instance.id.clone(),
                installed,
                warnings: plan.warnings.clone(),
            })
        }
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            emitter.emit("failed", None, Some(err.to_string()), true);
            Err(err)
        }
    }
}

fn commit(
    instance: &InstancePaths,
    plan: &InstallPlan,
    staging: &Path,
    transaction_id: &str,
) -> Result<Vec<InstalledMod>> {
    let mut manifest = mods::load_manifest(instance)?;
    let backup_dir = backups_root(instance).join(transaction_id);
    std::fs::create_dir_all(&backup_dir).map_err(|e| Error::io(&backup_dir, e))?;

    let mut journal = TransactionJournal {
        committed: false,
        targets: Vec::new(),
        backups: Vec::new(),
    };
    let journal_path = staging.join("journal.json");

    // Record every target up front, then persist the journal before any file
    // moves. A crash at any later point can undo the whole transaction.
    for item in &plan.items {
        let target = instance.mods.join(&item.filename);
        journal.targets.push(relative(instance, &target)?);
    }
    save_journal(&journal_path, &journal)?;

    let mut backed_up: HashSet<PathBuf> = HashSet::new();
    let prepare = (|| -> Result<()> {
        // Back up anything the transaction will replace: a previous version of
        // the same project, a file with the same name, or a disabled copy.
        for item in &plan.items {
            let mut candidates: Vec<PathBuf> = Vec::new();
            let active = instance.mods.join(&item.filename);
            if active.exists() {
                candidates.push(active.clone());
            }
            let disabled = instance.mods.join(format!("{}.disabled", item.filename));
            if disabled.exists() {
                candidates.push(disabled);
            }
            for entry in &manifest.mods {
                if entry.project_id == item.project_id || entry.file_name == item.filename {
                    let path = mods::disk_path(instance, entry);
                    if path.exists() {
                        candidates.push(path);
                    }
                }
            }

            for source in candidates {
                if !backed_up.insert(source.clone()) {
                    continue;
                }
                let name = source.file_name().unwrap_or_default();
                let dest = backup_dir.join(name);
                journal.backups.push(BackupMove {
                    backup: relative(instance, &dest)?,
                    original: relative(instance, &source)?,
                });
                // Persist each backup before moving the file, so recovery can
                // always tell an untouched original from a moved one.
                save_journal(&journal_path, &journal)?;
                move_file(&source, &dest)?;
            }
        }
        Ok(())
    })();

    if let Err(err) = prepare {
        mods::rollback_journal(instance, &journal);
        return Err(err);
    }

    let mut installed = Vec::new();
    let result = (|| -> Result<()> {
        for item in &plan.items {
            let source = staging.join(&item.filename);
            let target = instance.mods.join(&item.filename);
            if target.exists() && file_matches(&target, item)? {
                // Already exactly this artifact; keep it in place.
            } else {
                move_file(&source, &target)?;
            }
            manifest
                .mods
                .retain(|m| m.project_id != item.project_id && m.file_name != item.filename);
            let entry = InstalledMod {
                project_id: item.project_id.clone(),
                slug: item.slug.clone(),
                title: item.title.clone(),
                version_id: item.version_id.clone(),
                version_number: item.version_number.clone(),
                file_name: item.filename.clone(),
                sha1: Some(item.sha1.clone()).filter(|s| !s.is_empty()),
                sha512: Some(item.sha512.clone()).filter(|s| !s.is_empty()),
                size: item.size,
                installed_at: bench::now_epoch_ms(),
                feature_id: item.feature_id.clone(),
                source: "modrinth".into(),
                enabled: true,
                present: true,
                icon_url: item.icon_url.clone(),
                author: item.author.clone(),
                license: item.license.clone(),
            };
            manifest.mods.push(entry.clone());
            installed.push(entry);
        }
        mods::save_manifest(instance, &manifest)?;
        Ok(())
    })();

    if let Err(err) = result {
        mods::rollback_journal(instance, &journal);
        return Err(err);
    }

    journal.committed = true;
    let _ = save_journal(&journal_path, &journal);
    Ok(installed)
}

fn save_journal(path: &Path, journal: &TransactionJournal) -> Result<()> {
    let bytes =
        serde_json::to_vec_pretty(journal).map_err(|e| Error::json("transaction journal", e))?;
    std::fs::write(path, &bytes).map_err(|e| Error::io(path, e))
}

fn relative(instance: &InstancePaths, path: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(&instance.root)
        .map_err(|_| Error::msg(format!("path {} escaped the instance", path.display())))?;
    Ok(rel.to_string_lossy().to_string())
}

fn move_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|e| Error::io(to, e))?;
    std::fs::remove_file(from).map_err(|e| Error::io(from, e))?;
    Ok(())
}

fn file_matches(path: &Path, item: &PlannedMod) -> Result<bool> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return Ok(false),
    };
    if item.size > 0 && meta.len() != item.size {
        return Ok(false);
    }
    if !item.sha1.is_empty() && !mods::sha1_file(path)?.eq_ignore_ascii_case(&item.sha1) {
        return Ok(false);
    }
    if !item.sha512.is_empty() && !mods::sha512_file(path)?.eq_ignore_ascii_case(&item.sha512) {
        return Ok(false);
    }
    Ok(true)
}

fn allowed_download_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    matches!(
        parsed.host_str(),
        Some("cdn.modrinth.com") | Some("cdn-raw.modrinth.com")
    )
}

async fn download_one(item: &PlannedMod, dest: &Path, emitter: &ProgressEmitter) -> Result<()> {
    let mut last = None;
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(300 * (1 << attempt))).await;
        }
        match download_attempt(item, dest, emitter).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                tracing::warn!(
                    target: "mods",
                    "download attempt {}/3 for {} failed: {err}",
                    attempt + 1,
                    item.filename
                );
                let part = part_path(dest);
                let _ = tokio::fs::remove_file(&part).await;
                last = Some(err);
            }
        }
    }
    Err(last.unwrap_or_else(|| Error::msg(format!("could not download {}", item.filename))))
}

async fn download_attempt(item: &PlannedMod, dest: &Path, emitter: &ProgressEmitter) -> Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::io(parent, e))?;
    }
    let part = part_path(dest);
    let response = crate::http::client().get(&item.url).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::msg(format!(
            "GET {} failed with HTTP {status}",
            item.url
        )));
    }

    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|e| Error::io(&part, e))?;
    let mut sha1 = Sha1::new();
    let mut sha512 = Sha512::new();
    let mut received = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk)
            .await
            .map_err(|e| Error::io(&part, e))?;
        sha1.update(&chunk);
        sha512.update(&chunk);
        received += chunk.len() as u64;
        emitter
            .downloaded
            .fetch_add(chunk.len() as u64, Ordering::Relaxed);
        emitter.emit("downloading", Some(item.filename.clone()), None, false);
    }
    file.flush().await.map_err(|e| Error::io(&part, e))?;
    drop(file);

    if item.size > 0 && received != item.size {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(Error::msg(format!(
            "{}: expected {} bytes, got {received}",
            item.filename, item.size
        )));
    }
    let actual_sha1 = hex::encode(sha1.finalize());
    if !item.sha1.is_empty() && !actual_sha1.eq_ignore_ascii_case(&item.sha1) {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(Error::HashMismatch {
            path: dest.to_path_buf(),
            expected: item.sha1.clone(),
            actual: actual_sha1,
        });
    }
    let actual_sha512 = hex::encode(sha512.finalize());
    if !item.sha512.is_empty() && !actual_sha512.eq_ignore_ascii_case(&item.sha512) {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(Error::HashMismatch {
            path: dest.to_path_buf(),
            expected: item.sha512.clone(),
            actual: actual_sha512,
        });
    }
    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| Error::io(dest, e))?;
    Ok(())
}

fn part_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "mod.jar".to_string());
    dest.with_file_name(format!("{name}.part"))
}

struct ProgressEmitter {
    sink: ProgressFn,
    transaction_id: String,
    total_files: usize,
    total_bytes: u64,
    downloaded: Arc<AtomicU64>,
    completed: Arc<AtomicUsize>,
    last: Mutex<Instant>,
}

impl ProgressEmitter {
    fn emit(&self, phase: &str, current: Option<String>, message: Option<String>, force: bool) {
        if !force {
            let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
            if last.elapsed() < PROGRESS_INTERVAL {
                return;
            }
            *last = Instant::now();
        } else {
            *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        }
        (self.sink)(InstallProgress {
            transaction_id: self.transaction_id.clone(),
            phase: phase.to_string(),
            completed_files: self.completed.load(Ordering::Relaxed),
            total_files: self.total_files,
            downloaded_bytes: self
                .downloaded
                .load(Ordering::Relaxed)
                .min(self.total_bytes),
            total_bytes: self.total_bytes,
            current,
            message,
        });
    }
}

/// Read the jar's own metadata and reject a jar whose declared Minecraft range
/// excludes the instance. Modrinth filtering is the first gate; this is the
/// defence-in-depth check.
pub fn validate_jar_metadata(
    path: &Path,
    mc_version: &str,
    loader: Loader,
) -> Result<Option<String>> {
    let file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| Error::msg(format!("{} is not a valid jar: {e}", path.display())))?;

    if let Some(text) = read_entry(&mut archive, "fabric.mod.json") {
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| Error::json(format!("{} fabric.mod.json", path.display()), e))?;
        let id = value
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if let Some(depends) = value.get("depends").and_then(|d| d.get("minecraft")) {
            let specs = string_values(depends);
            if !specs.is_empty()
                && !specs
                    .iter()
                    .any(|spec| resolve::version_satisfies(spec, mc_version))
            {
                return Err(Error::msg(format!(
                    "{} declares Minecraft {} but this instance is {}.",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    specs.join(" || "),
                    mc_version
                )));
            }
        }
        return Ok(Some(id));
    }

    if let Some(text) = read_entry(&mut archive, "quilt.mod.json") {
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| Error::json(format!("{} quilt.mod.json", path.display()), e))?;
        let id = value
            .pointer("/quilt_loader/id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if let Some(depends) = value
            .pointer("/quilt_loader/depends")
            .and_then(|v| v.as_array())
        {
            for dep in depends {
                if dep.get("id").and_then(|v| v.as_str()) == Some("minecraft") {
                    if let Some(versions) = dep.get("versions") {
                        let specs = string_values(versions);
                        if !specs.is_empty()
                            && !specs
                                .iter()
                                .any(|spec| resolve::version_satisfies(spec, mc_version))
                        {
                            return Err(Error::msg(format!(
                                "{} declares Minecraft {} but this instance is {}.",
                                path.file_name().unwrap_or_default().to_string_lossy(),
                                specs.join(" || "),
                                mc_version
                            )));
                        }
                    }
                }
            }
        }
        return Ok(Some(id));
    }

    // Forge/NeoForge metadata is validated for presence; Modrinth's loader
    // filter and the file hashes remain the authoritative checks.
    let toml = if loader == Loader::NeoForge {
        read_entry(&mut archive, "META-INF/neoforge.mods.toml")
            .or_else(|| read_entry(&mut archive, "META-INF/mods.toml"))
    } else {
        read_entry(&mut archive, "META-INF/mods.toml")
            .or_else(|| read_entry(&mut archive, "META-INF/neoforge.mods.toml"))
    };
    if let Some(text) = toml {
        let id = text
            .lines()
            .find_map(|line| {
                let line = line.trim();
                line.strip_prefix("modId")
                    .and_then(|rest| rest.split_once('='))
                    .map(|(_, value)| value.trim().trim_matches('"').to_string())
            })
            .unwrap_or_default();
        return Ok(Some(id));
    }

    Ok(None)
}

fn read_entry(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut entry = archive.by_name(name).ok()?;
    if entry.size() > 2 * 1024 * 1024 {
        return None;
    }
    let mut text = String::new();
    entry.read_to_string(&mut text).ok()?;
    Some(text)
}

fn string_values(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_modrinth_hosts() {
        assert!(allowed_download_url(
            "https://cdn.modrinth.com/data/x/versions/y/file.jar"
        ));
        assert!(!allowed_download_url(
            "http://cdn.modrinth.com/data/x/versions/y/file.jar"
        ));
        assert!(!allowed_download_url("https://example.com/file.jar"));
        assert!(!allowed_download_url(
            "https://cdn.modrinth.com.evil.example/file.jar"
        ));
    }

    #[test]
    fn string_or_array_metadata() {
        assert_eq!(
            string_values(&serde_json::json!(">=1.21")),
            vec![">=1.21".to_string()]
        );
        assert_eq!(
            string_values(&serde_json::json!([">=1.21", "<1.22"])),
            vec![">=1.21".to_string(), "<1.22".to_string()]
        );
    }

    #[test]
    fn part_is_a_sibling() {
        assert_eq!(
            part_path(Path::new("/tmp/mods/a.jar")),
            PathBuf::from("/tmp/mods/a.jar.part")
        );
    }
}
