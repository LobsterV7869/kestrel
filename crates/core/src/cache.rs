//! Shared, content-addressed file store used by every profile.
//!
//! A small on-disk index records (size, mtime, sha1) so routine startups trust
//! the cache without re-hashing gigabytes. Hashes are recomputed only when the
//! size or mtime changed, which is exactly when a file may have been damaged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use futures_util::stream::{self, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};

use crate::download::{self, DownloadTask};
use crate::error::{Error, Result};
use crate::paths;

const INDEX_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub size: u64,
    pub mtime: u64,
    pub sha1: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileIndex {
    pub version: u32,
    pub entries: HashMap<String, IndexEntry>,
}

impl Default for FileIndex {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            entries: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct DownloadSummary {
    pub downloaded: usize,
    pub cached: usize,
}

pub struct SharedCache {
    index: Mutex<FileIndex>,
    index_path: PathBuf,
}

impl SharedCache {
    pub fn open() -> Result<Self> {
        std::fs::create_dir_all(paths::cache_dir())
            .map_err(|e| Error::io(paths::cache_dir(), e))?;
        let index_path = paths::cache_dir().join("file-index.json");
        let index = match std::fs::read(&index_path) {
            Ok(bytes) => serde_json::from_slice::<FileIndex>(&bytes).unwrap_or_default(),
            Err(_) => FileIndex::default(),
        };
        Ok(Self {
            index: Mutex::new(index),
            index_path,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FileIndex> {
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn index_path(&self) -> &Path {
        &self.index_path
    }

    pub fn is_valid(&self, task: &DownloadTask) -> Result<bool> {
        let meta = match std::fs::metadata(&task.dest) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(Error::io(&task.dest, err)),
        };
        if let Some(size) = task.size {
            if size > 0 && meta.len() != size {
                return Ok(false);
            }
        }
        let Some(expected) = task.sha1.as_deref() else {
            return Ok(true);
        };

        let key = task.dest.to_string_lossy().to_string();
        let mtime = mtime_secs(&meta);
        {
            let index = self.lock();
            if let Some(entry) = index.entries.get(&key) {
                if entry.size == meta.len()
                    && entry.mtime == mtime
                    && entry.sha1.eq_ignore_ascii_case(expected)
                {
                    return Ok(true);
                }
            }
        }

        let actual = download::sha1_file(&task.dest)?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Ok(false);
        }
        self.lock().entries.insert(
            key,
            IndexEntry {
                size: meta.len(),
                mtime,
                sha1: actual,
            },
        );
        Ok(true)
    }

    pub async fn ensure(&self, task: DownloadTask) -> Result<bool> {
        if self.is_valid(&task)? {
            return Ok(false);
        }
        download::download_file(&task).await?;
        self.note(&task.dest, task.sha1.as_deref())?;
        Ok(true)
    }

    pub async fn ensure_many(
        &self,
        tasks: Vec<DownloadTask>,
        concurrency: usize,
    ) -> Result<DownloadSummary> {
        if tasks.is_empty() {
            return Ok(DownloadSummary::default());
        }
        let concurrency = concurrency.clamp(1, 64);
        // Ask the cache first (cheap, mostly stat calls) so the summary and the
        // progress UI know what actually hit the network.
        let mut summary = DownloadSummary::default();
        let mut pending = Vec::new();
        for task in tasks {
            if self.is_valid(&task)? {
                summary.cached += 1;
            } else {
                pending.push(task);
            }
        }
        if pending.is_empty() {
            return Ok(summary);
        }
        let results = stream::iter(pending.into_iter().map(|task| async move {
            self.ensure(task).await
        }))
        .buffer_unordered(concurrency)
        .try_collect::<Vec<_>>()
        .await?;

        for downloaded in results {
            if downloaded {
                summary.downloaded += 1;
            } else {
                summary.cached += 1;
            }
        }
        Ok(summary)
    }

    /// Record a file we just wrote (or verified) without re-hashing it.
    pub fn note(&self, path: &Path, sha1: Option<&str>) -> Result<()> {
        let Ok(meta) = std::fs::metadata(path) else {
            return Ok(());
        };
        let sha1 = match sha1 {
            Some(sha1) => sha1.to_string(),
            None => download::sha1_file(path)?,
        };
        self.lock().entries.insert(
            path.to_string_lossy().to_string(),
            IndexEntry {
                size: meta.len(),
                mtime: mtime_secs(&meta),
                sha1,
            },
        );
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        let index = self.lock().clone();
        let bytes =
            serde_json::to_vec(&index).map_err(|e| Error::json("file-index", e))?;
        let tmp = self.index_path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| Error::io(&tmp, e))?;
        std::fs::rename(&tmp, &self.index_path)
            .map_err(|e| Error::io(&self.index_path, e))?;
        Ok(())
    }
}

fn mtime_secs(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn library_path(relative: &str) -> PathBuf {
    paths::shared_libraries_dir().join(relative)
}

pub fn object_path(hash: &str) -> PathBuf {
    let prefix = hash.get(0..2).unwrap_or("00");
    paths::shared_objects_dir().join(prefix).join(hash)
}

pub fn asset_url(hash: &str) -> String {
    let prefix = hash.get(0..2).unwrap_or("00");
    format!("{}/{prefix}/{hash}", crate::versions::ASSET_BASE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_paths_use_two_char_prefix() {
        let hash = "abcdef0123456789";
        assert!(object_path(hash).ends_with("ab/abcdef0123456789"));
        assert!(asset_url(hash).ends_with("/ab/abcdef0123456789"));
    }
}
