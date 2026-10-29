//! Streaming downloads with resume, SHA-1 verification and bounded retries.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use sha1::{Digest, Sha1};
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};
use crate::http;

const ATTEMPTS: u32 = 3;

#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub dest: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

impl DownloadTask {
    pub fn new(url: impl Into<String>, dest: impl Into<PathBuf>) -> Self {
        Self {
            url: url.into(),
            dest: dest.into(),
            sha1: None,
            size: None,
        }
    }

    pub fn with_sha1(mut self, sha1: impl Into<String>) -> Self {
        self.sha1 = Some(sha1.into());
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

pub async fn download_file(task: &DownloadTask) -> Result<u64> {
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(250 * (1 << attempt))).await;
        }
        match attempt_once(task).await {
            Ok(bytes) => return Ok(bytes),
            Err(err) => {
                tracing::warn!(
                    target: "download",
                    "attempt {}/{} for {} failed: {err}",
                    attempt + 1,
                    ATTEMPTS,
                    task.url
                );
                // A hash mismatch will not fix itself; fetch the file again from
                // scratch on the next attempt.
                if matches!(&err, Error::HashMismatch { .. }) {
                    let _ = tokio::fs::remove_file(part_path(&task.dest)).await;
                }
                last = Some(err);
            }
        }
    }
    Err(last.unwrap_or_else(|| Error::msg("download failed")))
}

async fn attempt_once(task: &DownloadTask) -> Result<u64> {
    if let Some(parent) = task.dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::io(parent, e))?;
    }

    let part = part_path(&task.dest);
    let existing = tokio::fs::metadata(&part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    let mut request = http::client().get(&task.url);
    if existing > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }
    let response = request.send().await?;
    let status = response.status();

    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // The partial file is already at least as large as the remote object.
        if let Some(size) = task.size {
            if existing == size {
                verify(&part, task)?;
                tokio::fs::rename(&part, &task.dest)
                    .await
                    .map_err(|e| Error::io(&task.dest, e))?;
                return Ok(0);
            }
        }
        let _ = tokio::fs::remove_file(&part).await;
        return Err(Error::msg(format!(
            "range not satisfiable for {} ({existing} bytes)",
            task.url
        )));
    }
    if !status.is_success() {
        return Err(Error::msg(format!("GET {} failed with HTTP {status}", task.url)));
    }

    let resuming = status == reqwest::StatusCode::PARTIAL_CONTENT && existing > 0;
    let mut file = if resuming {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .await
            .map_err(|e| Error::io(&part, e))?
    } else {
        tokio::fs::File::create(&part)
            .await
            .map_err(|e| Error::io(&part, e))?
    };

    let mut stream = response.bytes_stream();
    let mut received = if resuming { existing } else { 0 };
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk)
            .await
            .map_err(|e| Error::io(&part, e))?;
        received += chunk.len() as u64;
    }
    file.flush().await.map_err(|e| Error::io(&part, e))?;
    drop(file);

    if let Some(expected) = task.size {
        if received != expected {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(Error::msg(format!(
                "{}: expected {expected} bytes, got {received}",
                task.url
            )));
        }
    }

    verify(&part, task)?;
    tokio::fs::rename(&part, &task.dest)
        .await
        .map_err(|e| Error::io(&task.dest, e))?;
    Ok(received)
}

fn verify(path: &Path, task: &DownloadTask) -> Result<()> {
    let Some(expected) = task.sha1.as_deref() else {
        return Ok(());
    };
    let actual = sha1_file(path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(Error::HashMismatch {
            path: path.to_path_buf(),
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}

pub fn sha1_file(path: &Path) -> Result<String> {
    use std::io::Read;
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

pub fn sha1_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn file_matches(path: &Path, sha1: Option<&str>, size: Option<u64>) -> Result<bool> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(Error::io(path, err)),
    };
    if let Some(size) = size {
        if size > 0 && meta.len() != size {
            return Ok(false);
        }
    }
    match sha1 {
        None => Ok(true),
        Some(expected) => Ok(sha1_file(path)?.eq_ignore_ascii_case(expected)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_known_vector() {
        assert_eq!(sha1_bytes(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn part_path_is_sibling() {
        let path = Path::new("/tmp/a/b.jar");
        assert_eq!(part_path(path), Path::new("/tmp/a/b.jar.part"));
    }
}
