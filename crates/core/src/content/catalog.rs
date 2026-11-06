//! The Kestrel Essentials catalog.
//!
//! Candidate order is intentional: open-source projects come first, with
//! reputable all-rights-reserved projects only as a fallback when no compatible
//! open-source option exists. The resolver tries each candidate in order and
//! reports "unavailable" instead of installing something incompatible.

use futures_util::stream::{self, StreamExt};

use crate::error::Result;
use crate::mods::InstalledMod;
use crate::state::Loader;

use super::{loader_key, EssentialFeature, ModProvider, ModUpdate};

pub struct FeatureDef {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub keybind: &'static str,
    pub category: &'static str,
    pub essential: bool,
    pub candidates: &'static [&'static str],
}

pub const FEATURES: &[FeatureDef] = &[
    FeatureDef {
        id: "zoom",
        name: "Zoom",
        description: "Smooth, configurable zoom with a scroll-wheel strength control.",
        keybind: "C",
        category: "Movement",
        essential: true,
        candidates: &["zoomify", "ok-zoomer", "just-zoom"],
    },
    FeatureDef {
        id: "fullbright",
        name: "Fullbright",
        description: "Brightens the world without changing the server-side light level.",
        keybind: "Configurable",
        category: "Visuals",
        essential: true,
        candidates: &["gamma-utils", "fullbright-forge", "fullbright"],
    },
    FeatureDef {
        id: "toggle_sprint",
        name: "Toggle Sprint",
        description: "Hold-to-sprint becomes a toggle, with a configurable key.",
        keybind: "Configurable",
        category: "Movement",
        essential: true,
        candidates: &["toggle-sprint", "polysprint"],
    },
    FeatureDef {
        id: "toggle_sneak",
        name: "Toggle Sneak",
        description: "Toggle sneaking instead of holding the sneak key.",
        keybind: "Configurable",
        category: "Movement",
        essential: false,
        candidates: &["toggle-sneak-hotkey", "polysprint"],
    },
    FeatureDef {
        id: "crosshair",
        name: "Custom Crosshair",
        description: "Replace the vanilla crosshair with a configurable shape and colour.",
        keybind: "Configurable",
        category: "Visuals",
        essential: true,
        candidates: &["crosshair", "custom-crosshair-mod"],
    },
    FeatureDef {
        id: "auto_reconnect",
        name: "Auto Reconnect",
        description: "Reconnect to a server automatically after a disconnect.",
        keybind: "Configurable",
        category: "Utility",
        essential: false,
        // `autoreconnectrf` covers 1.21.x; the MIT fallbacks cover 1.20.1 and
        // older builds where it has no release. `autoreconnect` is the LGPL
        // original and is only reached when nothing else is compatible.
        candidates: &[
            "autoreconnectrf",
            "autoreconnector-fabric",
            "reconnect-mod-reloaded",
            "autorelog",
            "autoreconnect",
        ],
    },
    FeatureDef {
        id: "hide_totem",
        name: "Hide Totem Animation",
        description: "Skips the screen-covering totem pop animation.",
        keybind: "Configurable",
        category: "Visuals",
        essential: false,
        candidates: &["totemclean", "totem-norender"],
    },
    FeatureDef {
        id: "minimap",
        name: "Minimap",
        description: "A configurable minimap with terrain, entities and waypoint support.",
        keybind: "Configurable",
        category: "HUD",
        essential: true,
        candidates: &["immersive-minimaps", "xaeros-minimap", "axolotlclient"],
    },
    FeatureDef {
        id: "waypoints",
        name: "Waypoints",
        description: "Save, name and navigate back to important locations.",
        keybind: "B / J",
        category: "HUD",
        essential: true,
        candidates: &[
            "axolotlclient-waypoints",
            "xaeros-world-map",
            "xaeros-minimap",
        ],
    },
    FeatureDef {
        id: "keystrokes",
        name: "Keystrokes HUD",
        description: "Show movement and mouse input on screen with a custom layout.",
        keybind: "Configurable",
        category: "HUD",
        essential: true,
        candidates: &["clean-keystrokes", "evergreenhud"],
    },
    FeatureDef {
        id: "coordinates",
        name: "Coordinates HUD",
        description: "Always-visible coordinates with a configurable position.",
        keybind: "Configurable",
        category: "HUD",
        essential: true,
        candidates: &["coordinates-display", "evergreenhud"],
    },
    FeatureDef {
        id: "fps",
        name: "FPS Counter",
        description: "A lightweight frames-per-second readout on the HUD.",
        keybind: "Configurable",
        category: "HUD",
        essential: true,
        candidates: &["fps-monitor", "evergreenhud"],
    },
    FeatureDef {
        id: "ping",
        name: "Ping Display",
        description: "Server latency on the HUD with configurable position and colour.",
        keybind: "Configurable",
        category: "HUD",
        essential: true,
        candidates: &[
            "ping-counter",
            "better-ping-display-fabric",
            "better-ping-display",
        ],
    },
];

pub fn all() -> &'static [FeatureDef] {
    FEATURES
}

pub fn essentials() -> impl Iterator<Item = &'static FeatureDef> {
    FEATURES.iter().filter(|f| f.essential)
}

pub fn find(id: &str) -> Option<&'static FeatureDef> {
    FEATURES.iter().find(|f| f.id == id)
}

/// Resolve every feature for one instance, in parallel with a bounded fan-out.
pub async fn resolve_features(
    provider: &dyn ModProvider,
    mc_version: &str,
    loader: Loader,
    installed: &[InstalledMod],
) -> Result<Vec<EssentialFeature>> {
    if loader_key(loader).is_none() {
        return Ok(FEATURES
            .iter()
            .map(|def| {
                unavailable(
                    def,
                    "This instance is vanilla. Install Fabric or Quilt first.",
                )
            })
            .collect());
    }

    // Build the futures in a plain loop. An async closure here would make the
    // future's lifetime higher-ranked, which Tauri's command macro rejects.
    let mut futures = Vec::with_capacity(FEATURES.len());
    for def in FEATURES {
        futures.push(resolve_feature(
            provider, def, mc_version, loader, installed,
        ));
    }
    let resolved = stream::iter(futures)
        .buffer_unordered(6)
        .collect::<Vec<_>>()
        .await;

    // buffer_unordered loses the catalog order; restore it for a stable UI.
    let mut by_id: std::collections::HashMap<String, EssentialFeature> = resolved
        .into_iter()
        .map(|feature| (feature.id.clone(), feature))
        .collect();
    Ok(FEATURES
        .iter()
        .filter_map(|def| by_id.remove(def.id))
        .collect())
}

async fn resolve_feature(
    provider: &dyn ModProvider,
    def: &'static FeatureDef,
    mc_version: &str,
    loader: Loader,
    installed: &[InstalledMod],
) -> EssentialFeature {
    // An already-installed feature wins, even if the preferred candidate
    // changed since it was installed.
    if let Some(current) = installed.iter().find(|m| {
        m.feature_id.as_deref() == Some(def.id)
            || (!m.slug.is_empty() && def.candidates.contains(&m.slug.as_str()))
    }) {
        if current.project_id.is_empty() {
            return EssentialFeature {
                id: def.id.into(),
                name: def.name.into(),
                description: def.description.into(),
                keybind: def.keybind.into(),
                category: def.category.into(),
                essential: def.essential,
                available: true,
                installed: Some(current.clone()),
                reason: Some("Imported locally".into()),
                ..EssentialFeature::default()
            };
        }
        if let Ok(detail) = provider.project(&current.project_id).await {
            let versions = provider
                .versions(&current.project_id, mc_version, loader)
                .await
                .unwrap_or_default();
            let latest = versions.first().cloned();
            let update = latest.as_ref().and_then(|latest| {
                if latest.id == current.version_id {
                    None
                } else {
                    Some(ModUpdate {
                        project_id: current.project_id.clone(),
                        slug: current.slug.clone(),
                        title: current.title.clone(),
                        current_version: current.version_number.clone(),
                        current_version_id: current.version_id.clone(),
                        latest_version: latest.version_number.clone(),
                        latest_version_id: latest.id.clone(),
                        file_name: current.file_name.clone(),
                        icon_url: current.icon_url.clone(),
                    })
                }
            });
            return EssentialFeature {
                id: def.id.into(),
                name: def.name.into(),
                description: def.description.into(),
                keybind: def.keybind.into(),
                category: def.category.into(),
                essential: def.essential,
                project: Some(detail.summary),
                version: latest,
                available: true,
                installed: Some(current.clone()),
                update,
                ..EssentialFeature::default()
            };
        }

        // Offline or a provider hiccup: the mod is still installed, so keep
        // showing it instead of pretending the feature is missing.
        return EssentialFeature {
            id: def.id.into(),
            name: def.name.into(),
            description: def.description.into(),
            keybind: def.keybind.into(),
            category: def.category.into(),
            essential: def.essential,
            available: true,
            installed: Some(current.clone()),
            ..EssentialFeature::default()
        };
    }

    for candidate in def.candidates {
        let detail = match provider.project(candidate).await {
            Ok(detail) => detail,
            Err(_) => continue,
        };
        let versions = provider
            .versions(&detail.summary.id, mc_version, loader)
            .await
            .unwrap_or_default();
        let Some(latest) = versions.first().cloned() else {
            continue;
        };
        return EssentialFeature {
            id: def.id.into(),
            name: def.name.into(),
            description: def.description.into(),
            keybind: def.keybind.into(),
            category: def.category.into(),
            essential: def.essential,
            project: Some(detail.summary),
            version: Some(latest),
            available: true,
            ..EssentialFeature::default()
        };
    }

    unavailable(
        def,
        &format!(
            "No compatible version for Minecraft {mc_version} on {:?}.",
            loader
        ),
    )
}

fn unavailable(def: &FeatureDef, reason: &str) -> EssentialFeature {
    EssentialFeature {
        id: def.id.into(),
        name: def.name.into(),
        description: def.description.into(),
        keybind: def.keybind.into(),
        category: def.category.into(),
        essential: def.essential,
        available: false,
        reason: Some(reason.to_string()),
        ..EssentialFeature::default()
    }
}
