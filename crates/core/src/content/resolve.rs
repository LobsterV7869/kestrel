//! Compatibility filtering and dependency resolution.
//!
//! This module is pure apart from provider calls, so the tricky parts (version
//! ranges, cycles, duplicate files, missing dependencies) are unit-tested with
//! an in-memory provider.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::{Error, Result};
use crate::mods::InstalledMod;
use crate::state::Loader;

use super::{
    loader_key, loader_keys, InstallItem, InstallPlan, ModProvider, ModVersion, PlannedMod,
};

const MAX_MODS: usize = 64;

pub fn sanitize_file_name(name: &str) -> Result<String> {
    if name.is_empty() || name.len() > 240 {
        return Err(Error::msg("mod file name is empty or too long"));
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(Error::msg(format!("unsafe mod file name: {name}")));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(Error::msg("mod file name contains control characters"));
    }
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if !cleaned.to_lowercase().ends_with(".jar") {
        return Err(Error::msg(format!("mod file is not a jar: {name}")));
    }
    Ok(cleaned.trim_end_matches([' ', '.']).to_string())
}

/// Best-effort matcher for Fabric/Quilt `depends.minecraft` ranges. Modrinth's
/// loader/game-version filter is the primary gate; this catches jars whose
/// embedded metadata explicitly excludes the instance.
pub fn version_satisfies(spec: &str, version: &str) -> bool {
    let spec = spec.trim();
    if spec.is_empty() || spec == "*" {
        return true;
    }
    if spec.contains("${") {
        return true;
    }
    spec.split("||").any(|group| {
        group
            .split_whitespace()
            .filter(|token| !token.is_empty())
            .all(|token| matches_token(token, version))
    })
}

fn matches_token(token: &str, version: &str) -> bool {
    let token = token.trim().trim_end_matches('-');
    if token.is_empty() || token == "*" {
        return true;
    }
    if let Some(rest) = token.strip_prefix(">=") {
        return compare_versions(version, rest) != std::cmp::Ordering::Less;
    }
    if let Some(rest) = token.strip_prefix("<=") {
        return compare_versions(version, rest) != std::cmp::Ordering::Greater;
    }
    if let Some(rest) = token.strip_prefix('>') {
        return compare_versions(version, rest) == std::cmp::Ordering::Greater;
    }
    if let Some(rest) = token.strip_prefix('<') {
        return compare_versions(version, rest) == std::cmp::Ordering::Less;
    }
    if let Some(rest) = token.strip_prefix('=') {
        return prefix_matches(rest, version);
    }
    if let Some(rest) = token.strip_prefix('~') {
        return compare_versions(version, rest) != std::cmp::Ordering::Less
            && compare_versions(version, &bump_minor(rest)) == std::cmp::Ordering::Less;
    }
    if let Some(rest) = token.strip_prefix('^') {
        return compare_versions(version, rest) != std::cmp::Ordering::Less
            && compare_versions(version, &bump_major(rest)) == std::cmp::Ordering::Less;
    }
    prefix_matches(token, version)
}

fn prefix_matches(prefix: &str, version: &str) -> bool {
    let prefix = prefix.trim().trim_end_matches('-');
    if prefix.is_empty() || prefix == "*" {
        return true;
    }
    let p: Vec<&str> = prefix.split('.').collect();
    let v: Vec<&str> = version.split(['.', '-', '+']).collect();
    for (index, part) in p.iter().enumerate() {
        if matches!(*part, "x" | "X" | "*") {
            return true;
        }
        let Some(actual) = v.get(index) else {
            return false;
        };
        if part.parse::<u64>().ok() != actual.parse::<u64>().ok() {
            return false;
        }
    }
    true
}

fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let pa = numeric_parts(a);
    let pb = numeric_parts(b);
    let len = pa.len().max(pb.len());
    for index in 0..len {
        let av = pa.get(index).copied().unwrap_or(0);
        let bv = pb.get(index).copied().unwrap_or(0);
        match av.cmp(&bv) {
            std::cmp::Ordering::Equal => {}
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

fn numeric_parts(value: &str) -> Vec<u64> {
    value
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

fn bump_minor(value: &str) -> String {
    let mut parts = numeric_parts(value);
    while parts.len() < 2 {
        parts.push(0);
    }
    parts[1] += 1;
    parts.truncate(2);
    parts
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

fn bump_major(value: &str) -> String {
    let parts = numeric_parts(value);
    format!("{}.0.0", parts.first().copied().unwrap_or(0) + 1)
}

pub async fn plan(
    provider: &dyn ModProvider,
    instance_id: &str,
    mc_version: &str,
    loader: Loader,
    roots: &[InstallItem],
    installed: &[InstalledMod],
) -> Result<InstallPlan> {
    if loader_key(loader).is_none() {
        return Err(Error::msg(
            "This instance is vanilla. Install Fabric or Quilt before adding mods.",
        ));
    }
    if roots.is_empty() {
        return Err(Error::msg("No mods were selected."));
    }

    let installed_by_project: HashMap<&str, &InstalledMod> = installed
        .iter()
        .filter(|m| !m.project_id.is_empty())
        .map(|m| (m.project_id.as_str(), m))
        .collect();

    let mut queue: VecDeque<(InstallItem, bool)> =
        roots.iter().cloned().map(|item| (item, false)).collect();
    let mut selected: HashMap<String, PlannedMod> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut warnings = Vec::new();
    let mut warning_set = HashSet::new();
    let mut files: HashMap<String, String> = HashMap::new();

    while let Some((item, is_dependency)) = queue.pop_front() {
        if item.project_id.trim().is_empty() {
            continue;
        }
        if selected.len() >= MAX_MODS {
            return Err(Error::msg(format!(
                "This install would add more than {MAX_MODS} mods; refusing to continue."
            )));
        }

        // Already resolved in this transaction.
        if selected.contains_key(&item.project_id) {
            continue;
        }

        // A dependency that is already installed does not need to be fetched
        // again. Explicit version pins still trigger an update.
        if is_dependency {
            if let Some(existing) = installed_by_project.get(item.project_id.as_str()) {
                if item.version_id.is_none()
                    || item.version_id.as_deref() == Some(&existing.version_id)
                {
                    if !existing.enabled {
                        let warning = format!(
                            "{} is installed but disabled; the mod loader will ignore it.",
                            existing.title
                        );
                        if warning_set.insert(warning.clone()) {
                            warnings.push(warning);
                        }
                    }
                    continue;
                }
            }
        }

        let mut resolved = resolve_one(provider, &item, mc_version, loader).await?;
        resolved.planned.is_dependency = is_dependency;
        let key = resolved.planned.project_id.clone();
        if selected.contains_key(&key) {
            continue;
        }

        if let Some(existing_project) = files.get(&resolved.planned.filename) {
            if existing_project != &key {
                return Err(Error::msg(format!(
                    "Two mods would write the same file '{}'. Refusing to install.",
                    resolved.planned.filename
                )));
            }
        }
        files.insert(resolved.planned.filename.clone(), key.clone());

        for dependency in &resolved.version.dependencies {
            match dependency.dependency_type.as_str() {
                "required" => {
                    let Some(project_id) = dependency.project_id.clone() else {
                        let warning = format!(
                            "{} lists a required dependency without a project id.",
                            resolved.planned.title
                        );
                        if warning_set.insert(warning.clone()) {
                            warnings.push(warning);
                        }
                        continue;
                    };
                    if project_id == key {
                        continue;
                    }
                    queue.push_back((
                        InstallItem {
                            project_id,
                            version_id: dependency.version_id.clone(),
                            feature_id: resolved.planned.feature_id.clone(),
                        },
                        true,
                    ));
                }
                "optional" => {
                    let label = dependency
                        .project_id
                        .clone()
                        .or_else(|| dependency.file_name.clone())
                        .unwrap_or_else(|| "unknown".to_string());
                    let warning = format!(
                        "Optional dependency {label} of {} was not installed.",
                        resolved.planned.title
                    );
                    if warning_set.insert(warning.clone()) {
                        warnings.push(warning);
                    }
                }
                "incompatible" => {
                    let label = dependency
                        .project_id
                        .clone()
                        .or_else(|| dependency.file_name.clone())
                        .unwrap_or_else(|| "unknown".to_string());
                    let warning = format!(
                        "{} declares {label} as incompatible; it was not installed.",
                        resolved.planned.title
                    );
                    if warning_set.insert(warning.clone()) {
                        warnings.push(warning);
                    }
                }
                _ => {}
            }
        }

        order.push(key.clone());
        selected.insert(key, resolved.planned);
    }

    if selected.is_empty() {
        return Err(Error::msg("None of the selected mods could be resolved."));
    }

    let items = order
        .into_iter()
        .filter_map(|id| selected.remove(&id))
        .collect();

    Ok(InstallPlan {
        instance_id: instance_id.to_string(),
        mc_version: mc_version.to_string(),
        loader,
        items,
        warnings,
    })
}

struct Resolved {
    planned: PlannedMod,
    version: ModVersion,
}

async fn resolve_one(
    provider: &dyn ModProvider,
    item: &InstallItem,
    mc_version: &str,
    loader: Loader,
) -> Result<Resolved> {
    let versions = provider
        .versions(&item.project_id, mc_version, loader)
        .await?;
    if versions.is_empty() {
        return Err(Error::msg(format!(
            "No compatible version of '{}' exists for Minecraft {mc_version} on {:?}.",
            item.project_id, loader
        )));
    }

    let version = if let Some(version_id) = item.version_id.as_deref() {
        versions
            .iter()
            .find(|v| v.id == version_id)
            .cloned()
            .ok_or_else(|| {
                Error::msg(format!(
                    "The selected version of '{}' is not compatible with Minecraft {mc_version} on {:?}.",
                    item.project_id, loader
                ))
            })?
    } else {
        versions
            .first()
            .cloned()
            .ok_or_else(|| Error::msg("no compatible version"))?
    };

    if !version.game_versions.iter().any(|g| g == mc_version) {
        return Err(Error::msg(format!(
            "'{}' {} does not list Minecraft {mc_version}.",
            item.project_id, version.version_number
        )));
    }
    if !version
        .loaders
        .iter()
        .any(|l| loader_keys(loader).contains(&l.as_str()))
    {
        return Err(Error::msg(format!(
            "'{}' {} does not support {:?}.",
            item.project_id, version.version_number, loader
        )));
    }

    let file = version.primary_file().cloned().ok_or_else(|| {
        Error::msg(format!(
            "'{}' {} has no downloadable jar.",
            item.project_id, version.version_number
        ))
    })?;
    if file.sha1.is_empty() && file.sha512.is_empty() {
        return Err(Error::msg(format!(
            "'{}' {} has no checksum; refusing to install an unverifiable file.",
            item.project_id, version.version_number
        )));
    }
    if !file.url.starts_with("https://") {
        return Err(Error::msg(format!(
            "'{}' {} uses a non-HTTPS download URL.",
            item.project_id, version.version_number
        )));
    }
    let filename = sanitize_file_name(&file.filename)?;

    let detail = provider.project(&version.project_id).await.ok();
    let summary = detail.as_ref().map(|d| &d.summary);
    let license = detail
        .as_ref()
        .map(|d| d.license_name.clone())
        .or_else(|| summary.map(|s| s.license.clone()))
        .unwrap_or_default();

    Ok(Resolved {
        planned: PlannedMod {
            project_id: version.project_id.clone(),
            slug: summary
                .map(|s| s.slug.clone())
                .unwrap_or_else(|| item.project_id.clone()),
            title: summary
                .map(|s| s.title.clone())
                .unwrap_or_else(|| item.project_id.clone()),
            author: summary.map(|s| s.author.clone()).unwrap_or_default(),
            icon_url: summary.and_then(|s| s.icon_url.clone()),
            version_id: version.id.clone(),
            version_number: version.version_number.clone(),
            filename,
            url: file.url,
            sha1: file.sha1,
            sha512: file.sha512,
            size: file.size,
            license,
            feature_id: item.feature_id.clone(),
            is_dependency: false,
        },
        version,
    })
}

/// Build a plan for updating specific installed mods.
pub async fn update_plan(
    provider: &dyn ModProvider,
    instance_id: &str,
    mc_version: &str,
    loader: Loader,
    project_ids: &[String],
    installed: &[InstalledMod],
) -> Result<InstallPlan> {
    let roots: Vec<InstallItem> = project_ids
        .iter()
        .map(|project_id| InstallItem {
            project_id: project_id.clone(),
            version_id: None,
            feature_id: installed
                .iter()
                .find(|m| &m.project_id == project_id)
                .and_then(|m| m.feature_id.clone()),
        })
        .collect();
    let plan = plan(provider, instance_id, mc_version, loader, &roots, installed).await?;
    // `plan` already skips required dependencies that are installed, so the
    // returned items are exactly the requested updates plus any *new*
    // dependency the new version needs.
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{GalleryImage, ModDependency, ModFile, ProjectDetail, ProjectSummary};

    #[test]
    fn sanitizes_names() {
        assert_eq!(
            sanitize_file_name("zoomify-1.0.jar").unwrap(),
            "zoomify-1.0.jar"
        );
        assert!(sanitize_file_name("../evil.jar").is_err());
        assert!(sanitize_file_name("a/b.jar").is_err());
        assert_eq!(sanitize_file_name("a:b.jar").unwrap(), "a_b.jar");
    }

    #[test]
    fn matches_common_fabric_ranges() {
        assert!(version_satisfies("*", "1.21.1"));
        assert!(version_satisfies(">=1.20.1", "1.21.1"));
        assert!(version_satisfies(">=1.20.1 <1.22", "1.21.1"));
        assert!(!version_satisfies(">=1.22", "1.21.1"));
        assert!(version_satisfies("1.21.x", "1.21.1"));
        assert!(version_satisfies("~1.21", "1.21.1"));
        assert!(!version_satisfies("~1.20.1", "1.21.1"));
        assert!(version_satisfies("^1.20", "1.21.1"));
        assert!(!version_satisfies("^2", "1.21.1"));
        assert!(version_satisfies("1.21.1 || 1.22", "1.21.1"));
    }

    #[derive(Default)]
    struct Fake {
        projects: HashMap<String, ProjectDetail>,
        versions: HashMap<String, Vec<ModVersion>>,
    }

    impl ModProvider for Fake {
        fn project<'a>(
            &'a self,
            id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ProjectDetail>> + Send + 'a>>
        {
            let project = self.projects.get(id).cloned();
            Box::pin(
                async move { project.ok_or_else(|| Error::msg(format!("unknown project {id}"))) },
            )
        }

        fn versions<'a>(
            &'a self,
            id: &'a str,
            _mc_version: &'a str,
            _loader: Loader,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<ModVersion>>> + Send + 'a>>
        {
            let versions = self.versions.get(id).cloned().unwrap_or_default();
            Box::pin(async move { Ok(versions) })
        }
    }

    fn project(id: &str) -> ProjectDetail {
        ProjectDetail {
            summary: ProjectSummary {
                id: id.to_string(),
                slug: id.to_string(),
                title: id.to_string(),
                license: "MIT".into(),
                ..ProjectSummary::default()
            },
            gallery: Vec::<GalleryImage>::new(),
            license_name: "MIT".into(),
            ..ProjectDetail::default()
        }
    }

    fn version(id: &str, project_id: &str, deps: Vec<ModDependency>) -> ModVersion {
        ModVersion {
            id: format!("{id}-v"),
            project_id: project_id.to_string(),
            name: id.to_string(),
            version_number: "1.0.0".into(),
            version_type: "release".into(),
            game_versions: vec!["1.21.1".into()],
            loaders: vec!["fabric".into()],
            dependencies: deps,
            files: vec![ModFile {
                filename: format!("{id}.jar"),
                url: format!("https://cdn.modrinth.com/{id}.jar"),
                sha1: "abc".into(),
                sha512: "def".into(),
                size: 10,
                primary: true,
            }],
            date_published: "2025-01-01T00:00:00Z".into(),
        }
    }

    fn required(project_id: &str) -> ModDependency {
        ModDependency {
            project_id: Some(project_id.into()),
            version_id: None,
            dependency_type: "required".into(),
            file_name: None,
        }
    }

    #[tokio::test]
    async fn resolves_required_dependencies() {
        let mut fake = Fake::default();
        fake.projects.insert("a".into(), project("a"));
        fake.projects.insert("b".into(), project("b"));
        fake.versions
            .insert("a".into(), vec![version("a", "a", vec![required("b")])]);
        fake.versions
            .insert("b".into(), vec![version("b", "b", Vec::new())]);

        let plan = plan(
            &fake,
            "inst",
            "1.21.1",
            Loader::Fabric,
            &[InstallItem {
                project_id: "a".into(),
                version_id: None,
                feature_id: Some("zoom".into()),
            }],
            &[],
        )
        .await
        .unwrap();

        assert_eq!(plan.items.len(), 2);
        assert_eq!(plan.items[0].feature_id.as_deref(), Some("zoom"));
        assert_eq!(plan.items[1].feature_id.as_deref(), Some("zoom"));
    }

    #[tokio::test]
    async fn handles_cycles() {
        let mut fake = Fake::default();
        fake.projects.insert("a".into(), project("a"));
        fake.projects.insert("b".into(), project("b"));
        fake.versions
            .insert("a".into(), vec![version("a", "a", vec![required("b")])]);
        fake.versions
            .insert("b".into(), vec![version("b", "b", vec![required("a")])]);

        let plan = plan(
            &fake,
            "inst",
            "1.21.1",
            Loader::Fabric,
            &[InstallItem {
                project_id: "a".into(),
                version_id: None,
                feature_id: None,
            }],
            &[],
        )
        .await
        .unwrap();
        assert_eq!(plan.items.len(), 2);
    }

    #[tokio::test]
    async fn rejects_missing_compatible_version() {
        let fake = Fake::default();
        let err = plan(
            &fake,
            "inst",
            "1.21.1",
            Loader::Fabric,
            &[InstallItem {
                project_id: "missing".into(),
                version_id: None,
                feature_id: None,
            }],
            &[],
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("No compatible version"));
    }

    #[tokio::test]
    async fn skips_installed_dependencies() {
        let mut fake = Fake::default();
        fake.projects.insert("a".into(), project("a"));
        fake.projects.insert("b".into(), project("b"));
        fake.versions
            .insert("a".into(), vec![version("a", "a", vec![required("b")])]);
        fake.versions
            .insert("b".into(), vec![version("b", "b", Vec::new())]);

        let installed = vec![InstalledMod {
            project_id: "b".into(),
            title: "b".into(),
            version_id: "b-v".into(),
            enabled: true,
            present: true,
            ..InstalledMod::default()
        }];
        let plan = plan(
            &fake,
            "inst",
            "1.21.1",
            Loader::Fabric,
            &[InstallItem {
                project_id: "a".into(),
                version_id: None,
                feature_id: None,
            }],
            &installed,
        )
        .await
        .unwrap();
        assert_eq!(plan.items.len(), 1);
    }
}
