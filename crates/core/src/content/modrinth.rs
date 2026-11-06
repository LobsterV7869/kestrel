//! Modrinth API v2 client with a small on-disk response cache.
//!
//! The cache keeps the Mods page instant on repeat visits and lets the catalog
//! survive a brief network outage. It stores only public metadata, never files.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::download::sha1_bytes;
use crate::error::{Error, Result};
use crate::http;
use crate::paths;
use crate::state::Loader;

use super::{
    loader_keys, GalleryImage, ModDependency, ModFile, ModProvider, ModVersion, ProjectDetail,
    ProjectSummary, SearchResult,
};

pub const BASE: &str = "https://api.modrinth.com/v2";
const SEARCH_TTL: Duration = Duration::from_secs(10 * 60);
const PROJECT_TTL: Duration = Duration::from_secs(30 * 60);
const VERSIONS_TTL: Duration = Duration::from_secs(5 * 60);

pub struct ModrinthProvider;

impl ModProvider for ModrinthProvider {
    fn project<'a>(
        &'a self,
        id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ProjectDetail>> + Send + 'a>>
    {
        Box::pin(project(id))
    }

    fn versions<'a>(
        &'a self,
        id: &'a str,
        mc_version: &'a str,
        loader: Loader,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<ModVersion>>> + Send + 'a>>
    {
        Box::pin(versions(id, mc_version, loader))
    }
}

#[derive(Debug, Deserialize)]
struct RawSearch {
    hits: Vec<RawHit>,
    offset: u32,
    limit: u32,
    total_hits: u32,
}

#[derive(Debug, Deserialize)]
struct RawHit {
    project_id: String,
    slug: String,
    title: String,
    description: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    icon_url: Option<String>,
    #[serde(default)]
    display_categories: Vec<String>,
    #[serde(default)]
    versions: Vec<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    license: String,
    #[serde(default)]
    client_side: String,
    #[serde(default)]
    latest_version: Option<String>,
    #[serde(default)]
    featured: bool,
}

#[derive(Debug, Deserialize)]
struct RawProject {
    id: String,
    slug: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    icon_url: Option<String>,
    #[serde(default)]
    license: RawLicense,
    #[serde(default)]
    source_url: Option<String>,
    #[serde(default)]
    issues_url: Option<String>,
    #[serde(default)]
    gallery: Vec<RawGallery>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    client_side: String,
    #[serde(default)]
    loaders: Vec<String>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    downloads: u64,
}

#[derive(Debug, Default, Deserialize)]
struct RawLicense {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawGallery {
    url: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawMember {
    #[serde(default)]
    role: String,
    #[serde(default)]
    ordering: i64,
    user: RawUser,
}

#[derive(Debug, Deserialize)]
struct RawUser {
    #[serde(default)]
    username: String,
}

#[derive(Debug, Deserialize)]
struct RawVersion {
    id: String,
    project_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version_number: String,
    #[serde(default)]
    version_type: String,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
    #[serde(default)]
    dependencies: Vec<RawDependency>,
    #[serde(default)]
    files: Vec<RawFile>,
    #[serde(default)]
    date_published: String,
}

#[derive(Debug, Deserialize)]
struct RawDependency {
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    dependency_type: String,
    #[serde(default)]
    file_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    filename: String,
    url: String,
    #[serde(default)]
    hashes: RawHashes,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Default, Deserialize)]
struct RawHashes {
    #[serde(default)]
    sha1: Option<String>,
    #[serde(default)]
    sha512: Option<String>,
}

pub async fn search(
    query: &str,
    mc_version: &str,
    loader: Loader,
    category: &str,
    offset: u32,
    limit: u32,
) -> Result<SearchResult> {
    let mut facets: Vec<Vec<String>> = vec![vec!["project_type:mod".to_string()]];
    let keys = loader_keys(loader);
    if !keys.is_empty() {
        facets.push(keys.iter().map(|key| format!("categories:{key}")).collect());
    }
    if !mc_version.is_empty() {
        facets.push(vec![format!("versions:{mc_version}")]);
    }
    if !category.is_empty() && category != "all" && category != "featured" {
        facets.push(vec![format!("categories:{category}")]);
    }
    let facets = serde_json::to_string(&facets).map_err(|e| Error::json("modrinth facets", e))?;

    let index = if query.trim().is_empty() {
        "downloads"
    } else {
        "relevance"
    };
    let url = url_with(
        &format!("{BASE}/search"),
        &[
            ("query", query.trim().to_string()),
            ("limit", limit.clamp(1, 50).to_string()),
            ("offset", offset.to_string()),
            ("index", index.to_string()),
            ("facets", facets),
        ],
    )?;

    let raw: RawSearch = get_json_cached(&url, SEARCH_TTL).await?;
    Ok(SearchResult {
        hits: raw.hits.into_iter().map(summary_from_hit).collect(),
        offset: raw.offset,
        limit: raw.limit,
        total_hits: raw.total_hits,
    })
}

pub async fn project(id: &str) -> Result<ProjectDetail> {
    let url = format!("{BASE}/project/{}", encode_path(id));
    let raw: RawProject = get_json_cached(&url, PROJECT_TTL).await?;

    let author = match get_json_cached::<Vec<RawMember>>(
        &format!("{BASE}/project/{}/members", encode_path(&raw.id)),
        PROJECT_TTL,
    )
    .await
    {
        Ok(members) => members
            .into_iter()
            .min_by_key(|m| {
                let owner = if m.role.eq_ignore_ascii_case("owner") {
                    0
                } else {
                    1
                };
                (owner, m.ordering)
            })
            .map(|m| m.user.username)
            .unwrap_or_else(|| "Modrinth".to_string()),
        Err(_) => "Modrinth".to_string(),
    };

    Ok(ProjectDetail {
        summary: ProjectSummary {
            id: raw.id,
            slug: raw.slug,
            title: raw.title,
            description: raw.description,
            author,
            icon_url: raw.icon_url,
            categories: Vec::new(),
            loaders: raw.loaders,
            game_versions: raw.game_versions,
            downloads: raw.downloads,
            license: raw.license.id.clone(),
            client_side: raw.client_side,
            latest_version: None,
            featured: false,
        },
        body: raw.body,
        gallery: raw
            .gallery
            .into_iter()
            .map(|g| GalleryImage {
                url: g.url,
                title: g.title,
                description: g.description,
            })
            .collect(),
        source_url: raw.source_url,
        issues_url: raw.issues_url,
        license_name: if raw.license.name.is_empty() {
            raw.license.id
        } else {
            raw.license.name
        },
        license_url: raw.license.url,
    })
}

pub async fn versions(id: &str, mc_version: &str, loader: Loader) -> Result<Vec<ModVersion>> {
    let keys = loader_keys(loader);
    let mut query: Vec<(&str, String)> = Vec::new();
    if !keys.is_empty() {
        query.push((
            "loaders",
            serde_json::to_string(keys).map_err(|e| Error::json("modrinth loaders", e))?,
        ));
    }
    if !mc_version.is_empty() {
        query.push((
            "game_versions",
            serde_json::to_string(&[mc_version])
                .map_err(|e| Error::json("modrinth versions", e))?,
        ));
    }
    let url = url_with(
        &format!("{BASE}/project/{}/version", encode_path(id)),
        &query,
    )?;
    let raw: Vec<RawVersion> = get_json_cached(&url, VERSIONS_TTL).await?;

    let mut out: Vec<ModVersion> = raw
        .into_iter()
        .filter(|v| v.files.iter().any(|f| f.filename.ends_with(".jar")))
        .filter(|v| {
            (mc_version.is_empty() || v.game_versions.iter().any(|g| g == mc_version))
                && (keys.is_empty() || v.loaders.iter().any(|l| keys.contains(&l.as_str())))
        })
        .map(version_from_raw)
        .collect();
    out.sort_by(|a, b| b.date_published.cmp(&a.date_published));
    Ok(out)
}

fn summary_from_hit(hit: RawHit) -> ProjectSummary {
    ProjectSummary {
        id: hit.project_id,
        slug: hit.slug,
        title: hit.title,
        description: hit.description,
        author: hit.author,
        icon_url: hit.icon_url,
        categories: hit.display_categories,
        loaders: Vec::new(),
        game_versions: hit.versions,
        downloads: hit.downloads,
        license: hit.license,
        client_side: hit.client_side,
        latest_version: hit.latest_version,
        featured: hit.featured,
    }
}

fn version_from_raw(raw: RawVersion) -> ModVersion {
    ModVersion {
        id: raw.id,
        project_id: raw.project_id,
        name: raw.name,
        version_number: raw.version_number,
        version_type: raw.version_type,
        game_versions: raw.game_versions,
        loaders: raw.loaders,
        dependencies: raw
            .dependencies
            .into_iter()
            .map(|d| ModDependency {
                project_id: d.project_id,
                version_id: d.version_id,
                dependency_type: d.dependency_type,
                file_name: d.file_name,
            })
            .collect(),
        files: raw
            .files
            .into_iter()
            .map(|f| ModFile {
                filename: f.filename,
                url: f.url,
                sha1: f.hashes.sha1.unwrap_or_default(),
                sha512: f.hashes.sha512.unwrap_or_default(),
                size: f.size,
                primary: f.primary,
            })
            .collect(),
        date_published: raw.date_published,
    }
}

fn encode_path(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn url_with(base: &str, query: &[(&str, String)]) -> Result<String> {
    let mut url = reqwest::Url::parse(base)
        .map_err(|e| Error::msg(format!("invalid Modrinth URL {base}: {e}")))?;
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    Ok(url.into())
}

async fn fetch_bytes(url: &str) -> Result<bytes::Bytes> {
    let mut last = None;
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
        }
        let response = http::client().get(url).send().await?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            last = Some(Error::msg("Modrinth rate limit reached"));
            continue;
        }
        if !status.is_success() {
            return Err(Error::msg(format!("GET {url} failed with HTTP {status}")));
        }
        return Ok(response.bytes().await?);
    }
    Err(last.unwrap_or_else(|| Error::msg("Modrinth request failed")))
}

async fn get_json_cached<T: DeserializeOwned>(url: &str, ttl: Duration) -> Result<T> {
    let key = sha1_bytes(url.as_bytes());
    let dir = paths::cache_dir().join("modrinth");
    let path = dir.join(format!("{key}.json"));

    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|age| age < ttl)
        .unwrap_or(false);
    if fresh {
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                return Ok(value);
            }
        }
    }

    match fetch_bytes(url).await {
        Ok(bytes) => {
            if let Err(err) = std::fs::create_dir_all(&dir) {
                tracing::debug!(target: "modrinth", "cache dir: {err}");
            } else {
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, &bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
            serde_json::from_slice(&bytes).map_err(|e| Error::json(url.to_string(), e))
        }
        Err(err) => match std::fs::read(&path) {
            Ok(bytes) => {
                tracing::warn!(target: "modrinth", "using stale cache for {url}: {err}");
                serde_json::from_slice(&bytes).map_err(|e| Error::json(url.to_string(), e))
            }
            Err(_) => Err(err),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_path_segments() {
        assert_eq!(encode_path("zoomify"), "zoomify");
        assert_eq!(encode_path("a b"), "a%20b");
        assert_eq!(encode_path("../x"), "..%2Fx");
    }

    #[test]
    fn builds_query_url() {
        let url = url_with(
            "https://api.modrinth.com/v2/search",
            &[("query", "a b".into())],
        )
        .unwrap();
        assert!(url.contains("query=a+b") || url.contains("query=a%20b"));
    }
}
