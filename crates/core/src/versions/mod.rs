//! Mojang version manifest and version JSON, including the rule system that
//! decides which libraries, natives and arguments apply to this machine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::http;

pub const ASSET_BASE: &str = "https://resources.download.minecraft.net";
pub const LIBRARY_BASE: &str = "https://libraries.minecraft.net/";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VersionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub time: String,
    pub release_time: String,
    pub sha1: String,
    #[serde(default)]
    pub compliance_level: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    pub main_class: String,
    #[serde(default)]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(default)]
    pub assets: Option<String>,
    #[serde(default)]
    pub downloads: VersionDownloads,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default)]
    pub java_version: Option<JavaVersion>,
    #[serde(default)]
    pub arguments: Option<Arguments>,
    #[serde(default)]
    pub minecraft_arguments: Option<String>,
    #[serde(default)]
    pub logging: Option<Logging>,
    #[serde(default)]
    pub compliance_level: Option<u32>,
    #[serde(default)]
    pub release_time: Option<String>,
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default)]
    pub minimum_launcher_version: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionDownloads {
    #[serde(default)]
    pub client: Option<Artifact>,
    #[serde(default)]
    pub server: Option<Artifact>,
    #[serde(default)]
    pub client_mappings: Option<Artifact>,
    #[serde(default)]
    pub server_mappings: Option<Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    pub url: String,
}

impl Artifact {
    /// Relative path inside the libraries/ or versions/ store.
    pub fn path(&self) -> Result<&str> {
        self.path
            .as_deref()
            .ok_or_else(|| Error::msg(format!("artifact {} has no path", self.url)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexRef {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    #[serde(default)]
    pub total_size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetIndex {
    #[serde(default)]
    pub objects: HashMap<String, AssetObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    #[serde(default)]
    pub component: Option<String>,
    #[serde(default)]
    pub major_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
    #[serde(default)]
    pub rules: Option<Vec<Rule>>,
    #[serde(default)]
    pub extract: Option<Extract>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub action: Action,
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Conditional {
        #[serde(default)]
        rules: Option<Vec<Rule>>,
        value: StringOrVec,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrVec {
    One(String),
    Many(Vec<String>),
}

impl StringOrVec {
    pub fn as_slice(&self) -> &[String] {
        match self {
            StringOrVec::One(s) => std::slice::from_ref(s),
            StringOrVec::Many(v) => v,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.as_slice().iter().map(|s| s.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Logging {
    #[serde(default)]
    pub client: Option<LoggingClient>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoggingClient {
    #[serde(default)]
    pub argument: Option<String>,
    #[serde(default)]
    pub file: Option<LoggingFile>,
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingFile {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Copy)]
pub struct Platform {
    pub os: &'static str,
    pub arch: &'static str,
}

impl Platform {
    pub fn current() -> Self {
        Self {
            os: if cfg!(target_os = "windows") {
                "windows"
            } else if cfg!(target_os = "macos") {
                "osx"
            } else {
                "linux"
            },
            arch: if cfg!(target_arch = "x86") {
                "x86"
            } else if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                "x86_64"
            },
        }
    }

    /// Mojang's natives classifier uses 32/64 rather than the OS arch name.
    pub fn bits(&self) -> &'static str {
        if self.arch == "x86" {
            "32"
        } else {
            "64"
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuleContext {
    pub platform: Platform,
    pub features: HashMap<String, bool>,
}

impl RuleContext {
    pub fn current() -> Self {
        Self {
            platform: Platform::current(),
            features: HashMap::new(),
        }
    }

    pub fn with_features(mut self, features: impl IntoIterator<Item = (String, bool)>) -> Self {
        self.features.extend(features);
        self
    }
}

impl Rule {
    pub fn matches(&self, ctx: &RuleContext) -> bool {
        if let Some(os) = &self.os {
            if let Some(name) = &os.name {
                if name != ctx.platform.os {
                    return false;
                }
            }
            if let Some(arch) = &os.arch {
                if arch != ctx.platform.arch {
                    return false;
                }
            }
            // `version` is a regex against the OS kernel version; in practice it
            // only appears in old macOS-only rules, so ignoring it is safe here.
        }
        if let Some(features) = &self.features {
            for (key, want) in features {
                if ctx.features.get(key).copied().unwrap_or(false) != *want {
                    return false;
                }
            }
        }
        true
    }
}

/// Mojang rule semantics: with no rules, allow. Otherwise start disallowed and
/// let every matching rule overwrite the decision, in order.
pub fn rules_allow(rules: Option<&[Rule]>, ctx: &RuleContext) -> bool {
    match rules {
        None => true,
        Some([]) => true,
        Some(rules) => {
            let mut allowed = false;
            for rule in rules {
                if rule.matches(ctx) {
                    allowed = rule.action == Action::Allow;
                }
            }
            allowed
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedLibrary {
    pub name: String,
    pub artifact: Option<Artifact>,
    pub native: Option<Artifact>,
    pub extract: Option<Extract>,
}

/// Parse a Maven coordinate (`group:artifact:version[:classifier][@ext]`) into
/// the relative repository path used by the official launcher.
pub fn maven_path(coordinate: &str) -> Result<String> {
    let (coordinate, ext) = match coordinate.split_once('@') {
        Some((c, ext)) => (c, ext),
        None => (coordinate, "jar"),
    };
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 {
        return Err(Error::msg(format!(
            "invalid Maven coordinate '{coordinate}'"
        )));
    }
    let (group, artifact, version) = (parts[0], parts[1], parts[2]);
    let classifier = parts.get(3).copied();
    let mut file = format!("{artifact}-{version}");
    if let Some(classifier) = classifier {
        file.push('-');
        file.push_str(classifier);
    }
    file.push('.');
    file.push_str(ext);
    Ok(format!(
        "{}/{}/{}/{}",
        group.replace('.', "/"),
        artifact,
        version,
        file
    ))
}

fn synthesized(library: &Library, path: String) -> Artifact {
    let base = library.url.clone().unwrap_or_else(|| LIBRARY_BASE.to_string());
    Artifact {
        url: format!("{}/{path}", base.trim_end_matches('/')),
        path: Some(path),
        sha1: None,
        size: None,
    }
}

pub fn resolve_libraries(libraries: &[Library], ctx: &RuleContext) -> Vec<ResolvedLibrary> {
    let mut out = Vec::with_capacity(libraries.len());
    for library in libraries {
        if !rules_allow(library.rules.as_deref(), ctx) {
            continue;
        }

        let artifact = match library
            .downloads
            .as_ref()
            .and_then(|d| d.artifact.clone())
        {
            Some(artifact) => Some(artifact),
            None => {
                if library
                    .downloads
                    .as_ref()
                    .and_then(|d| d.classifiers.as_ref())
                    .is_some()
                {
                    None
                } else {
                    maven_path(&library.name).ok().map(|path| synthesized(library, path))
                }
            }
        };

        let native = library.natives.as_ref().and_then(|natives| {
            let template = natives.get(ctx.platform.os)?;
            let classifier = template.replace("${arch}", ctx.platform.bits());
            if let Some(artifact) = library
                .downloads
                .as_ref()
                .and_then(|d| d.classifiers.as_ref())
                .and_then(|c| c.get(&classifier))
            {
                return Some(artifact.clone());
            }
            let coordinate = format!("{}:{}", library.name, classifier);
            maven_path(&coordinate)
                .ok()
                .map(|path| synthesized(library, path))
        });

        if artifact.is_none() && native.is_none() {
            continue;
        }
        out.push(ResolvedLibrary {
            name: library.name.clone(),
            artifact,
            native,
            extract: library.extract.clone(),
        });
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct LibraryPaths {
    pub classpath: Vec<PathBuf>,
    pub natives: Vec<(Artifact, Option<Extract>)>,
}

impl VersionJson {
    pub fn library_paths(
        &self,
        ctx: &RuleContext,
        libraries_root: &Path,
    ) -> Result<LibraryPaths> {
        let mut out = LibraryPaths::default();
        let mut seen = std::collections::HashSet::new();
        for library in resolve_libraries(&self.libraries, ctx) {
            if let Some(artifact) = library.artifact {
                let path = libraries_root.join(artifact.path()?);
                if seen.insert(path.clone()) {
                    out.classpath.push(path);
                }
            }
            if let Some(native) = library.native {
                out.natives.push((native, library.extract));
            }
        }
        Ok(out)
    }

    /// Required Java major version, falling back to the modern default.
    pub fn java_major(&self) -> u32 {
        self.java_version
            .as_ref()
            .map(|j| j.major_version)
            .unwrap_or(17)
    }
}

/// Expand a list of game/JVM arguments with `${placeholder}` substitution and
/// rule filtering.
pub fn expand_arguments(
    arguments: &[Argument],
    ctx: &RuleContext,
    values: &HashMap<String, String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for argument in arguments {
        let (rules, value) = match argument {
            Argument::Plain(value) => (None, StringOrVec::One(value.clone())),
            Argument::Conditional { rules, value } => (rules.as_deref(), value.clone()),
        };
        if !rules_allow(rules, ctx) {
            continue;
        }
        for piece in value.iter() {
            out.push(substitute(piece, values));
        }
    }
    out
}

pub fn substitute(input: &str, values: &HashMap<String, String>) -> String {
    let mut out = input.to_string();
    for (key, value) in values {
        let needle = format!("${{{key}}}");
        if out.contains(&needle) {
            out = out.replace(&needle, value);
        }
    }
    out
}

/// Legacy versions (pre-1.13) use a single space-separated string.
pub fn expand_legacy(arguments: &str, values: &HashMap<String, String>) -> Vec<String> {
    arguments
        .split_whitespace()
        .map(|part| substitute(part, values))
        .collect()
}

/// Fetch the manifest from Piston. The caller caches it on disk; this function
/// only owns the network call.
pub async fn fetch_manifest() -> Result<VersionManifest> {
    http::get_json(http::PISTON_MANIFEST_URL).await
}

pub async fn fetch_version_json(url: &str) -> Result<VersionJson> {
    http::get_json(url).await
}

pub fn find_version<'a>(manifest: &'a VersionManifest, id: &str) -> Option<&'a VersionEntry> {
    manifest.versions.iter().find(|v| v.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(os: &'static str, arch: &'static str) -> RuleContext {
        RuleContext {
            platform: Platform { os, arch },
            features: HashMap::new(),
        }
    }

    fn rule(action: Action, os: Option<&str>) -> Rule {
        Rule {
            action,
            os: os.map(|name| OsRule {
                name: Some(name.into()),
                version: None,
                arch: None,
            }),
            features: None,
        }
    }

    #[test]
    fn no_rules_allow() {
        assert!(rules_allow(None, &ctx("linux", "x86_64")));
        assert!(rules_allow(Some(&[]), &ctx("linux", "x86_64")));
    }

    #[test]
    fn allow_only_matching_os() {
        let rules = vec![rule(Action::Allow, Some("windows"))];
        assert!(rules_allow(Some(&rules), &ctx("windows", "x86_64")));
        assert!(!rules_allow(Some(&rules), &ctx("linux", "x86_64")));
    }

    #[test]
    fn later_rule_overrides_earlier() {
        let rules = vec![
            rule(Action::Allow, None),
            rule(Action::Disallow, Some("osx")),
        ];
        assert!(rules_allow(Some(&rules), &ctx("linux", "x86_64")));
        assert!(!rules_allow(Some(&rules), &ctx("osx", "x86_64")));
    }

    #[test]
    fn feature_rules() {
        let rules = vec![Rule {
            action: Action::Allow,
            os: None,
            features: Some(HashMap::from([("is_demo_user".into(), true)])),
        }];
        assert!(!rules_allow(Some(&rules), &ctx("linux", "x86_64")));
        let mut demo = ctx("linux", "x86_64");
        demo.features.insert("is_demo_user".into(), true);
        assert!(rules_allow(Some(&rules), &demo));
    }

    #[test]
    fn maven_coordinates() {
        assert_eq!(
            maven_path("net.java.jinput:jinput:2.0.5").unwrap(),
            "net/java/jinput/jinput/2.0.5/jinput-2.0.5.jar"
        );
        assert_eq!(
            maven_path("org.lwjgl:lwjgl:3.3.1:natives-linux").unwrap(),
            "org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-linux.jar"
        );
        assert_eq!(
            maven_path("com.example:lib:1.0@zip").unwrap(),
            "com/example/lib/1.0/lib-1.0.zip"
        );
    }

    #[test]
    fn substitution() {
        let values = HashMap::from([
            ("auth_player_name".to_string(), "Steve".to_string()),
            ("game_directory".to_string(), "/games/mc".to_string()),
        ]);
        assert_eq!(
            substitute("--username ${auth_player_name}", &values),
            "--username Steve"
        );
        assert_eq!(
            substitute("--gameDir ${game_directory}", &values),
            "--gameDir /games/mc"
        );
    }

    #[test]
    fn legacy_arguments_split() {
        let values = HashMap::from([("auth_player_name".to_string(), "Alex".to_string())]);
        assert_eq!(
            expand_legacy("--username ${auth_player_name} --version 1.6.4", &values),
            vec!["--username", "Alex", "--version", "1.6.4"]
        );
    }
}
