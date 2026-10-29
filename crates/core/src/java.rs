//! JVM detection. Stage 4 adds downloading managed runtimes; this module always
//! answers "what is already on this machine?" without touching the network.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::paths;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaInstall {
    pub path: PathBuf,
    pub major: u32,
    pub version: String,
    /// `system`, `jdk`, `managed` or `JAVA_HOME`.
    pub source: String,
}

/// Parse the first `version "..."` line from `java -version` output.
pub fn parse_version(output: &str) -> Option<(u32, String)> {
    for line in output.lines() {
        if let Some(index) = line.find("version \"") {
            let rest = &line[index + "version \"".len()..];
            let end = rest.find('"')?;
            let version = &rest[..end];
            return Some((major_of(version), version.to_string()));
        }
    }
    None
}

fn major_of(version: &str) -> u32 {
    let mut parts = version.split(['.', '-', '+', '_']);
    let first = parts.next().unwrap_or_default();
    if first == "1" {
        parts.next().and_then(|s| s.parse().ok()).unwrap_or(8)
    } else {
        first.parse().unwrap_or(0)
    }
}

pub fn probe(path: &Path) -> Option<JavaInstall> {
    if !path.is_file() {
        return None;
    }
    let output = Command::new(path).arg("-version").output().ok()?;
    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    let (major, version) = parse_version(&text)?;
    if major == 0 {
        return None;
    }
    Some(JavaInstall {
        path: path.to_path_buf(),
        major,
        version,
        source: "system".to_string(),
    })
}

fn java_in(dir: &Path) -> PathBuf {
    dir.join("bin").join(if cfg!(windows) { "java.exe" } else { "java" })
}

fn push_candidate(out: &mut Vec<(PathBuf, String)>, path: PathBuf, source: &str) {
    if path.is_file() {
        out.push((path, source.to_string()));
    }
}

fn scan_parent(
    out: &mut Vec<(PathBuf, String)>,
    parent: &Path,
    java: &Path,
    source: &str,
    depth: usize,
) {
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let path = entry.path().join(java);
            if path.is_file() {
                out.push((path, source.to_string()));
            } else if depth > 0 {
                scan_parent(out, &entry.path(), java, source, depth - 1);
            }
        }
    }
}

/// Every distinct JVM we can find, best (highest major) first.
pub fn detect_all() -> Vec<JavaInstall> {
    let java = if cfg!(windows) { "java.exe" } else { "java" };
    let mut candidates: Vec<(PathBuf, String)> = Vec::new();

    if let Ok(home) = std::env::var("JAVA_HOME") {
        push_candidate(&mut candidates, java_in(Path::new(&home)), "JAVA_HOME");
    }

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            push_candidate(&mut candidates, dir.join(java), "system");
        }
    }

    let bases = [
        PathBuf::from("/usr/lib/jvm"),
        PathBuf::from("/usr/java"),
        PathBuf::from("/opt/java"),
        paths::shared_java_dir(),
    ];
    for base in bases {
        scan_parent(&mut candidates, &base, Path::new("bin").join(java).as_path(), "jdk", 2);
    }
    if let Some(home) = dirs::home_dir() {
        scan_parent(
            &mut candidates,
            &home.join(".jdks"),
            Path::new("bin").join(java).as_path(),
            "jdk",
            1,
        );
    }

    let mut seen = std::collections::HashSet::new();
    let mut found = Vec::new();
    for (path, source) in candidates {
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(key) {
            continue;
        }
        if let Some(mut install) = probe(&path) {
            install.source = source;
            found.push(install);
        }
    }
    found.sort_by(|a, b| b.major.cmp(&a.major).then_with(|| a.path.cmp(&b.path)));
    found
}

/// Prefer the exact major the version asks for, else the closest newer JVM,
/// else the newest available.
pub fn find_for(required: u32) -> Option<JavaInstall> {
    let all = detect_all();
    all.iter()
        .find(|java| java.major == required)
        .or_else(|| all.iter().filter(|java| java.major > required).min_by_key(|java| java.major))
        .or_else(|| all.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_version() {
        let text = "openjdk version \"17.0.20\" 2025-07-15\nOpenJDK Runtime Environment";
        assert_eq!(parse_version(text), Some((17, "17.0.20".into())));
    }

    #[test]
    fn parses_legacy_version() {
        let text = "java version \"1.8.0_402\"\nJava(TM) SE Runtime Environment";
        assert_eq!(parse_version(text), Some((8, "1.8.0_402".into())));
    }

    #[test]
    fn parses_early_access() {
        let text = "openjdk version \"21-ea\" 2023-09-19";
        assert_eq!(parse_version(text), Some((21, "21-ea".into())));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_version("not a jvm"), None);
    }
}
